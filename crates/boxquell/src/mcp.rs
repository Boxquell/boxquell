//! `boxquell mcp`: a Model Context Protocol server on standard input and output.
//!
//! It lets an assistant on this computer work with sandboxes: list them,
//! create and start them, run commands in them and read the network log.
//! Each tool runs the same `boxquell` command a person would type, so there is
//! one set of rules. Changing network rules and deleting sandboxes are off
//! unless the server is started with the flag for it; secrets have no tool at all.

use std::io::{BufRead, Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};

/// What the client may do beyond the defaults.
#[derive(Debug, Clone, Copy, Default)]
pub struct Access {
    /// Nothing that changes a sandbox: no create, start, stop, run or install.
    pub read_only: bool,
    /// Allow and deny destinations. Off by default: opening the network is a person's decision.
    pub network_rules: bool,
    /// Delete sandboxes.
    pub delete: bool,
}

/// The newest protocol revision this server was written against. A client
/// that asks for another one gets its own back; the messages used here are
/// the same in all of them.
const PROTOCOL: &str = "2025-06-18";
/// A command's output beyond this is cut at the front; the end usually says what went wrong.
const OUTPUT_LIMIT: usize = 60_000;
const DEFAULT_TIMEOUT: u64 = 120;
const LONGEST_TIMEOUT: u64 = 1800;

struct Tool {
    name: &'static str,
    description: &'static str,
    /// JSON schema properties, and which of them are required.
    properties: Value,
    required: &'static [&'static str],
    kind: Kind,
}

#[derive(PartialEq, Clone, Copy)]
enum Kind {
    Read,
    Change,
    NetworkRule,
    Delete,
}

fn tools() -> Vec<Tool> {
    let name = json!({ "type": "string", "description": "Name of the sandbox" });
    vec![
        Tool { name: "list_sandboxes", description: "List the sandboxes on this computer with their state.", properties: json!({}), required: &[], kind: Kind::Read },
        Tool {
            name: "sandbox_info",
            description: "Everything about one sandbox: state, system, resources, installed software, network mode and rules, ports, shared folders, warnings.",
            properties: json!({ "name": name }),
            required: &["name"],
            kind: Kind::Read,
        },
        Tool {
            name: "list_software",
            description: "The agents, languages, tools and IDE servers that can be installed into a sandbox, with their ids and versions.",
            properties: json!({}),
            required: &[],
            kind: Kind::Read,
        },
        Tool {
            name: "network_log",
            description: "Recent connection attempts of a sandbox, newest last: destination, port, whether it was allowed or blocked and by which rule. Use it to find out why something in the sandbox cannot reach the network.",
            properties: json!({
                "name": name,
                "blocked_only": { "type": "boolean", "description": "Only blocked attempts" },
                "lines": { "type": "integer", "minimum": 1, "maximum": 500, "description": "How many entries (default 50)" },
            }),
            required: &["name"],
            kind: Kind::Read,
        },
        Tool {
            name: "create_sandbox",
            description: "Create a sandbox: a microVM whose network is closed except for what is allowed. The project folder appears in it as /workspace. It is not started.",
            properties: json!({
                "name": { "type": "string", "description": "Lowercase letters, digits and dashes" },
                "workspace": { "type": "string", "description": "Folder on this computer to share as /workspace (full path)" },
                "os": { "type": "string", "description": "Operating system id, default debian" },
                "software": { "type": "array", "items": { "type": "string" }, "description": "Ids from list_software to install, e.g. [\"claude\", \"node@24\"]" },
                "cpus": { "type": "integer", "minimum": 1 },
                "ram_mib": { "type": "integer", "minimum": 256 },
                "disk_gib": { "type": "integer", "minimum": 1 },
            }),
            required: &["name"],
            kind: Kind::Change,
        },
        Tool { name: "start_sandbox", description: "Start a sandbox.", properties: json!({ "name": name }), required: &["name"], kind: Kind::Change },
        Tool { name: "stop_sandbox", description: "Stop a sandbox. Its disk and settings stay.", properties: json!({ "name": name }), required: &["name"], kind: Kind::Change },
        Tool {
            name: "run_command",
            description: "Run a shell command inside a sandbox as its user, in the project root, and return its output and exit status. The sandbox is started when needed. The command sees only the sandbox: its own files, /workspace, and the network destinations that are allowed.",
            properties: json!({
                "name": name,
                "command": { "type": "string", "description": "A bash command line" },
                "timeout_seconds": { "type": "integer", "minimum": 1, "maximum": LONGEST_TIMEOUT, "description": "Give up after this long (default 120)" },
            }),
            required: &["name", "command"],
            kind: Kind::Change,
        },
        Tool {
            name: "install_software",
            description: "Install software from list_software into a sandbox. What a recipe needs is installed with it.",
            properties: json!({ "name": name, "software": { "type": "array", "items": { "type": "string" }, "minItems": 1 } }),
            required: &["name", "software"],
            kind: Kind::Change,
        },
        Tool {
            name: "allow_destination",
            description: "Allow a sandbox to reach a destination: a host name, `*.example.com`, an address or a network, optionally with `:port`.",
            properties: json!({ "name": name, "destination": { "type": "string" } }),
            required: &["name", "destination"],
            kind: Kind::NetworkRule,
        },
        Tool {
            name: "deny_destination",
            description: "Block a destination for a sandbox explicitly.",
            properties: json!({ "name": name, "destination": { "type": "string" } }),
            required: &["name", "destination"],
            kind: Kind::NetworkRule,
        },
        Tool {
            name: "delete_sandbox",
            description: "Delete a sandbox with its disk. The shared project folder on this computer is not touched.",
            properties: json!({ "name": name }),
            required: &["name"],
            kind: Kind::Delete,
        },
    ]
}

fn offered(tool: &Tool, access: Access) -> bool {
    match tool.kind {
        Kind::Read => true,
        Kind::Change => !access.read_only,
        Kind::NetworkRule => !access.read_only && access.network_rules,
        Kind::Delete => !access.read_only && access.delete,
    }
}

/// Serve requests until standard input ends.
pub fn serve(access: Access) -> Result<()> {
    let exe = std::env::current_exe().context("cannot find the boxquell program")?;
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let answer = match serde_json::from_str::<Value>(&line) {
            Ok(message) => handle(&message, access, &exe),
            Err(err) => Some(json!({ "jsonrpc": "2.0", "id": null, "error": { "code": -32700, "message": format!("not JSON: {err}") } })),
        };
        if let Some(answer) = answer {
            // One message per line; the client reads until the newline.
            writeln!(stdout, "{answer}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// The answer to one message; `None` for notifications, which get none.
fn handle(message: &Value, access: Access, exe: &PathBuf) -> Option<Value> {
    let id = message.get("id").filter(|id| !id.is_null())?.clone();
    let method = message["method"].as_str().unwrap_or_default();
    let params = &message["params"];
    let result = match method {
        "initialize" => Ok(json!({
            "protocolVersion": params["protocolVersion"].as_str().unwrap_or(PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "boxquell", "title": "Boxquell", "version": env!("CARGO_PKG_VERSION") },
            "instructions": "Boxquell runs coding agents in microVM sandboxes whose network is closed by default. Use run_command to work inside a sandbox. When something there cannot reach the network, look at network_log; opening a destination is the user's decision unless allow_destination is offered.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": tools().iter().filter(|tool| offered(tool, access)).map(|tool| json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": { "type": "object", "properties": tool.properties, "required": tool.required },
            })).collect::<Vec<_>>(),
        })),
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or_default();
            match tools().into_iter().find(|tool| tool.name == name && offered(tool, access)) {
                // A tool that fails answers with its error as text, so the model can read it and react.
                Some(tool) => Ok(match call(&tool, &params["arguments"], exe) {
                    Ok(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": false }),
                    Err(err) => json!({ "content": [{ "type": "text", "text": format!("{err:#}") }], "isError": true }),
                }),
                None => Err((-32602, format!("unknown tool {name:?}"))),
            }
        }
        other => Err((-32601, format!("method {other:?} is not supported"))),
    };
    Some(match result {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

/// A text argument that will be passed to `boxquell` as one argument of its own.
/// It must not look like an option.
fn text<'a>(arguments: &'a Value, key: &str) -> Result<&'a str> {
    let value = arguments[key].as_str().with_context(|| format!("the argument `{key}` is missing"))?.trim();
    ensure!(!value.is_empty(), "the argument `{key}` is empty");
    ensure!(!value.starts_with('-'), "`{key}` must not start with a dash");
    Ok(value)
}

fn sandbox(arguments: &Value) -> Result<&str> {
    let name = text(arguments, "name")?;
    boxquell_core::spec::validate_name(name)?;
    Ok(name)
}

fn list(arguments: &Value, key: &str) -> Result<Vec<String>> {
    let Some(items) = arguments[key].as_array() else { return Ok(Vec::new()) };
    items
        .iter()
        .map(|item| {
            let item = item.as_str().with_context(|| format!("`{key}` must be a list of texts"))?.trim().to_string();
            ensure!(!item.is_empty() && !item.starts_with('-'), "an entry of `{key}` is empty or starts with a dash");
            Ok(item)
        })
        .collect()
}

fn call(tool: &Tool, arguments: &Value, exe: &PathBuf) -> Result<String> {
    let run = |args: Vec<String>, timeout: u64| boxquell(exe, &args, Duration::from_secs(timeout));
    let words = |words: &[&str]| words.iter().map(|word| word.to_string()).collect::<Vec<String>>();
    match tool.name {
        "list_sandboxes" => run(words(&["ls"]), 30),
        "list_software" => run(words(&["recipes"]), 30),
        "sandbox_info" => {
            let home = boxquell_core::Home::locate()?;
            let mut info = crate::client::Client::connect(&home)?.get(&format!("/api/sandboxes/{}", sandbox(arguments)?))?;
            // Of no use to a model: a drawing, and where the key to the sandbox lies.
            for key in ["os_icon", "os_icon_color", "ssh_key"] {
                info.as_object_mut().map(|fields| fields.remove(key));
            }
            Ok(serde_json::to_string_pretty(&info)?)
        }
        "network_log" => {
            let mut args = words(&["logs", sandbox(arguments)?, "--lines"]);
            args.push(arguments["lines"].as_u64().unwrap_or(50).clamp(1, 500).to_string());
            if arguments["blocked_only"] == true {
                args.push("--blocked".into());
            }
            run(args, 30)
        }
        "create_sandbox" => {
            let mut args = words(&["new", sandbox(arguments)?]);
            if !arguments["workspace"].is_null() {
                let folder = PathBuf::from(text(arguments, "workspace")?);
                ensure!(folder.is_absolute() && folder.is_dir(), "{} is not a folder on this computer (give its full path)", folder.display());
                args.extend(["--workspace".into(), folder.display().to_string()]);
            }
            if !arguments["os"].is_null() {
                args.extend(["--os".into(), text(arguments, "os")?.to_string()]);
            }
            for (key, flag) in [("cpus", "--cpus"), ("ram_mib", "--ram"), ("disk_gib", "--disk")] {
                if let Some(number) = arguments[key].as_u64() {
                    args.extend([flag.to_string(), number.to_string()]);
                }
            }
            let software = list(arguments, "software")?;
            if !software.is_empty() {
                args.extend(["--with".into(), software.join(",")]);
            }
            // Installing software and, the first time, fetching the system takes a while.
            run(args, LONGEST_TIMEOUT)
        }
        "start_sandbox" => run(words(&["up", sandbox(arguments)?]), 300),
        "stop_sandbox" => run(words(&["down", sandbox(arguments)?]), 120),
        "run_command" => {
            let command = arguments["command"].as_str().context("the argument `command` is missing")?;
            ensure!(!command.trim().is_empty(), "the argument `command` is empty");
            let timeout = arguments["timeout_seconds"].as_u64().unwrap_or(DEFAULT_TIMEOUT).clamp(1, LONGEST_TIMEOUT);
            run(vec!["run".into(), sandbox(arguments)?.into(), "--".into(), "bash".into(), "-lc".into(), command.into()], timeout)
        }
        "install_software" => {
            let software = list(arguments, "software")?;
            ensure!(!software.is_empty(), "name at least one piece of software");
            let mut args = words(&["add", sandbox(arguments)?]);
            args.extend(software);
            run(args, LONGEST_TIMEOUT)
        }
        "allow_destination" => run(words(&["allow", sandbox(arguments)?, text(arguments, "destination")?]), 30),
        "deny_destination" => run(words(&["deny", sandbox(arguments)?, text(arguments, "destination")?]), 30),
        "delete_sandbox" => run(words(&["rm", sandbox(arguments)?]), 120),
        other => bail!("unknown tool {other}"),
    }
}

/// Run `boxquell` with these arguments and return what it printed. A command
/// that ends with an error is an error here too, with its output as the text.
fn boxquell(exe: &PathBuf, args: &[String], timeout: Duration) -> Result<String> {
    let mut child = Command::new(exe).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    // Read both streams while it runs; a full pipe would stall the command.
    let collect = |mut stream: Box<dyn Read + Send>| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = stream.read_to_end(&mut bytes);
            bytes
        })
    };
    let out = collect(Box::new(child.stdout.take().context("no output stream")?));
    let err = collect(Box::new(child.stderr.take().context("no error stream")?));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break Some(status);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut output = String::from_utf8_lossy(&out.join().unwrap_or_default()).into_owned();
    let errors = String::from_utf8_lossy(&err.join().unwrap_or_default()).into_owned();
    if !errors.trim().is_empty() {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&errors);
    }
    if output.len() > OUTPUT_LIMIT {
        let mut cut = output.len() - OUTPUT_LIMIT;
        while !output.is_char_boundary(cut) {
            cut += 1;
        }
        output = format!("[the first {cut} bytes of output were left out]\n{}", &output[cut..]);
    }
    match status {
        None => bail!("{output}\n[stopped after {} seconds without finishing]", timeout.as_secs()),
        Some(status) if !status.success() => {
            bail!("{}\n[exit status {}]", output.trim_end(), status.code().map_or("unknown".to_string(), |code| code.to_string()))
        }
        Some(_) => Ok(if output.trim().is_empty() { "done".to_string() } else { output }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(message: Value, access: Access) -> Value {
        handle(&message, access, &PathBuf::from("boxquell")).expect("an answer")
    }

    #[test]
    fn offers_risky_tools_only_on_request() {
        let names = |access: Access| -> Vec<String> {
            let answer = ask(json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }), access);
            answer["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap().to_string()).collect()
        };
        let default = names(Access::default());
        assert!(default.contains(&"run_command".to_string()));
        assert!(!default.contains(&"allow_destination".to_string()));
        assert!(!default.contains(&"delete_sandbox".to_string()));
        let open = names(Access { network_rules: true, delete: true, read_only: false });
        assert!(open.contains(&"allow_destination".to_string()) && open.contains(&"delete_sandbox".to_string()));
        let reading = names(Access { read_only: true, network_rules: true, delete: true });
        assert_eq!(reading, ["list_sandboxes", "sandbox_info", "list_software", "network_log"]);
    }

    #[test]
    fn refuses_tools_that_are_not_offered_and_arguments_that_look_like_options() {
        let call = |name: &str, arguments: Value| {
            ask(json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": { "name": name, "arguments": arguments } }), Access::default())
        };
        assert_eq!(call("delete_sandbox", json!({ "name": "box" }))["error"]["code"], -32602);
        let answer = call("start_sandbox", json!({ "name": "--help" }));
        assert_eq!(answer["result"]["isError"], true);
    }

    #[test]
    fn answers_the_handshake_and_ignores_notifications() {
        let answer =
            ask(json!({ "jsonrpc": "2.0", "id": 0, "method": "initialize", "params": { "protocolVersion": "2024-11-05" } }), Access::default());
        assert_eq!(answer["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(answer["result"]["serverInfo"]["name"], "boxquell");
        assert!(handle(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }), Access::default(), &PathBuf::from("boxquell")).is_none());
    }
}
