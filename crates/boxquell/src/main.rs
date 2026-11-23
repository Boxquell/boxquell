//! boxquell: run coding agents in microVMs whose network access you control.

mod api;
mod client;
mod daemon;
mod doctor;
mod install;
mod ssh;
mod terminal;
mod wizard;

use anyhow::{bail, Context, Result};
use boxquell_core::drive::shell_quote;
use boxquell_core::recipe::Catalog;
use boxquell_core::repo;
use boxquell_core::{Backend, Home, SecretDef};
use clap::{Parser, Subcommand, ValueEnum};
use client::Client;
use serde_json::{json, Value};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser)]
#[command(name = "boxquell", version, about = "Sandboxes for coding agents with an explicit network allowlist")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    /// Only allowed destinations are reachable
    Whitelist,
    /// Everything public is reachable except denied destinations
    Blacklist,
}

impl ModeArg {
    fn name(self) -> &'static str {
        match self {
            Self::Whitelist => "whitelist",
            Self::Blacklist => "blacklist",
        }
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum BackendArg {
    Krun,
    Qemu,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a sandbox
    New {
        /// Leave out to pick everything in an interactive form
        name: Option<String>,
        /// Host directory to share at /workspace (default: none)
        #[arg(short, long)]
        workspace: Option<PathBuf>,
        #[arg(long, default_value_t = 2)]
        cpus: u8,
        /// Memory in MiB
        #[arg(long, default_value_t = 4096)]
        ram: u32,
        #[arg(long)]
        backend: Option<BackendArg>,
        /// Network mode
        #[arg(long, value_enum, default_value = "whitelist")]
        mode: ModeArg,
        /// Destination to allow: example.com, *.example.com:443, 192.0.2.7 or 10.0.0.0/8 (repeatable)
        #[arg(long = "allow", value_name = "HOST[:PORT]")]
        allow: Vec<String>,
        /// Host secret to make usable in the sandbox (repeatable)
        #[arg(long = "secret", value_name = "NAME")]
        secrets: Vec<String>,
        /// Disk size in GiB
        #[arg(long, default_value_t = 20)]
        disk: u32,
        /// Recipes to install right away, e.g. `--with claude,python` (see `boxquell recipes`)
        #[arg(long = "with", value_name = "RECIPE", value_delimiter = ',')]
        with: Vec<String>,
    },
    /// Start a sandbox
    Up { name: String },
    /// Stop a sandbox
    Down { name: String },
    /// Freeze a running sandbox: it keeps its memory and programs but uses no CPU
    Pause { name: String },
    /// Continue a paused sandbox
    Resume { name: String },
    /// Delete a sandbox and its disk
    Rm { name: String },
    /// List sandboxes
    Ls,
    /// Check that this host can run sandboxes
    Doctor,
    /// Open the web UI for all sandboxes in the browser
    Ui {
        /// Only print the address
        #[arg(long)]
        no_open: bool,
    },
    /// List what can be installed into a sandbox (agents, languages, tools)
    Recipes,
    /// Manage recipe repositories: git repositories that add agents, languages and tools
    Repo {
        #[command(subcommand)]
        command: RepoCmd,
    },
    /// Install recipes into a sandbox, e.g. `boxquell add box claude python@3.13`
    Add {
        name: String,
        #[arg(required = true)]
        recipes: Vec<String>,
    },
    /// Open a shell in a sandbox, starting it if needed
    Shell { name: String },
    /// Run a command (for example an agent) in a sandbox's /workspace
    Run {
        name: String,
        #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
        command: Vec<String>,
    },
    /// Allow a destination
    Allow { name: String, host: String },
    /// Block a destination explicitly
    Deny { name: String, host: String },
    /// Remove a rule, e.g. `boxquell unrule box "allow example.com"`
    Unrule { name: String, rule: String },
    /// Rules for every sandbox: denies no sandbox can undo, allows every sandbox gets
    Global {
        #[command(subcommand)]
        command: GlobalCmd,
    },
    /// Change CPUs, memory or disk size; used from the sandbox's next start
    Set {
        name: String,
        #[arg(long)]
        cpus: Option<u8>,
        /// Memory in MiB
        #[arg(long)]
        ram: Option<u32>,
        /// Disk size in GiB; a disk can only grow
        #[arg(long)]
        disk: Option<u32>,
    },
    /// Forward ports of this computer into a sandbox, e.g. to open a web app running in it
    Port {
        #[command(subcommand)]
        command: PortCmd,
    },
    /// Give a sandbox Claude Code settings, e.g. for a LiteLLM gateway; keys in them stay on this computer
    ClaudeSettings {
        name: String,
        /// Settings file (default: this computer's managed settings)
        file: Option<PathBuf>,
        /// Take the settings away again
        #[arg(long, conflicts_with = "file")]
        clear: bool,
    },
    /// Switch a sandbox between whitelist and blacklist
    Mode {
        name: String,
        #[arg(value_enum)]
        mode: ModeArg,
    },
    /// Show a sandbox's network rules
    Policy { name: String },
    /// Show the network log
    Logs {
        name: Option<String>,
        /// Keep printing new entries
        #[arg(short, long)]
        follow: bool,
        /// Only blocked attempts
        #[arg(short, long)]
        blocked: bool,
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: u32,
    },
    /// Manage credentials that stay on the host
    Secret {
        #[command(subcommand)]
        command: SecretCmd,
    },
    /// Run or stop the background daemon
    Daemon {
        /// Stop the running daemon and all sandboxes
        #[arg(long)]
        stop: bool,
    },
    /// Internal: become the VM process
    #[command(hide = true)]
    VmRun { library: PathBuf, spec: String },
}

#[derive(Subcommand)]
enum GlobalCmd {
    /// Allow a destination in every sandbox
    Allow { host: String },
    /// Block a destination in every sandbox
    Deny { host: String },
    /// Remove a global rule, e.g. `boxquell global unrule "deny example.com"`
    Unrule { rule: String },
    /// List the global rules
    Ls,
}

#[derive(Subcommand)]
enum PortCmd {
    /// `boxquell port add box 3000` or `boxquell port add box 8080:3000` (this computer:sandbox)
    Add {
        name: String,
        ports: String,
    },
    /// Remove the forward from this computer's port
    Rm {
        name: String,
        host: u16,
    },
    Ls {
        name: String,
    },
}

#[derive(Subcommand)]
enum RepoCmd {
    /// Add a repository by its git URL (or a local path)
    Add {
        url: String,
        /// Name used as the prefix of its recipes (default: from the URL)
        #[arg(long)]
        name: Option<String>,
    },
    /// List added repositories
    Ls,
    /// Fetch the latest recipes of one repository, or of all
    Update { name: Option<String> },
    /// Remove a repository
    Rm { name: String },
}

#[derive(Subcommand)]
enum SecretCmd {
    /// Store a secret; the value is read from the terminal or stdin
    Set {
        /// A known service (anthropic, claude-oauth, openai, openrouter, github) or your own name
        name: String,
        /// Store it for this sandbox only instead of sharing it
        #[arg(long)]
        sandbox: Option<String>,
        /// Host the secret may be sent to (repeatable; required for own names)
        #[arg(long = "host")]
        hosts: Vec<String>,
        /// Environment variable that carries the placeholder in the sandbox (repeatable)
        #[arg(long = "env")]
        env: Vec<String>,
        #[arg(long)]
        placeholder: Option<String>,
    },
    /// List secrets (never their values)
    Ls,
    Rm {
        name: String,
        /// Remove the secret of this sandbox
        #[arg(long)]
        sandbox: Option<String>,
    },
    /// Make a shared secret usable in a sandbox
    Attach { sandbox: String, name: String },
}

/// The recipe catalog; on first use this fetches the official repositories.
fn catalog(home: &Home) -> Result<Catalog> {
    for problem in repo::ensure_official(home) {
        eprintln!("warning: {problem}");
    }
    Catalog::of(home)
}

fn rule(action: &str, host: &str) -> Value {
    json!({ "rule": format!("{action} {host}") })
}

fn clock(ts_ms: u64) -> String {
    let secs = ts_ms / 1000 % 86_400;
    format!("{:02}:{:02}:{:02}Z", secs / 3600, secs / 60 % 60, secs % 60)
}

fn print_event(event: &Value) {
    let text = |key: &str| event[key].as_str().unwrap_or("");
    let verdict = if event["allowed"] == true { "allow" } else { "BLOCK" };
    let mut target = text("host").to_string();
    if target.is_empty() {
        target = text("ip").to_string();
    }
    if let Some(port) = event["port"].as_u64() {
        target.push_str(&format!(":{port}"));
    }
    let detail = match text("kind") {
        "http" => format!(
            "{} {} -> {}{}",
            text("method"),
            text("path"),
            event["status"],
            match text("secret") {
                "" => String::new(),
                secret => format!(" [secret {secret}]"),
            }
        ),
        "tcp" if text("phase") == "close" => {
            format!("closed, {} B up, {} B down, {} ms; {}", event["bytes_up"], event["bytes_down"], event["duration_ms"], text("reason"))
        }
        _ => text("reason").to_string(),
    };
    println!("{} {:<12} {verdict} {:<4} {target}  {detail}", clock(event["ts_ms"].as_u64().unwrap_or(0)), text("sandbox"), text("kind"));
}

/// Start the sandbox if needed and replace this process with an ssh session into it.
fn ssh_into(client: &Client, name: &str, remote: Option<String>) -> Result<()> {
    let info = client.post(&format!("/api/sandboxes/{name}/start"), json!({}))?;
    let key = PathBuf::from(info["ssh_key"].as_str().context("daemon reported no ssh key")?);
    let port = info["ssh_port"].as_u64().context("daemon reported no ssh port")? as u16;
    let mut command = Command::new("ssh");
    command.args(ssh::base_args(&key, port));
    // Interactive use needs a terminal on the other side; BatchMode stays on, there are no prompts.
    if std::io::stdin().is_terminal() {
        command.arg("-t");
    }
    command.args(remote);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec()).context("cannot run ssh; is OpenSSH installed?")
    }
    #[cfg(not(unix))]
    {
        let status = command.status().context("cannot run ssh; is OpenSSH installed?")?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Cmd::VmRun { library, spec } = &cli.command {
        // On success the process has become the VM and never gets here.
        #[cfg(unix)]
        match boxquell_vm::krun::enter(library, &serde_json::from_str(spec)?)? {}
        #[cfg(not(unix))]
        {
            let _ = (library, spec);
            bail!("the libkrun backend is not available on this system");
        }
    }
    let home = Home::locate()?;
    match cli.command {
        Cmd::VmRun { .. } => unreachable!("handled above"),
        Cmd::Daemon { stop: true } => match Client::existing(&home) {
            Some(client) => client.post("/api/shutdown", json!({})).map(drop)?,
            None => println!("the daemon is not running"),
        },
        Cmd::Daemon { stop: false } => {
            if Client::existing(&home).is_some() {
                bail!("the boxquell daemon is already running");
            }
            let runtime = tokio::runtime::Runtime::new()?;
            let served = runtime.block_on(daemon::serve(home));
            // Everything is stopped and saved by now; do not wait for helper
            // threads that are still blocked reading from a closed terminal.
            runtime.shutdown_timeout(std::time::Duration::from_secs(2));
            served?;
        }
        Cmd::New { name: None, allow, secrets, mode, .. } => {
            if !std::io::stdin().is_terminal() {
                bail!("give the sandbox a name: `boxquell new <name>` (the interactive form needs a terminal)");
            }
            let catalog = catalog(&home)?;
            let Some(choice) = wizard::run(&catalog)? else { return Ok(()) };
            let rules: Vec<String> = allow.iter().map(|host| format!("allow {host}")).collect();
            let spec = json!({ "name": choice.name, "workspace": choice.workspace, "cpus": choice.cpus, "ram_mib": choice.ram_mib,
                "disk_gib": choice.disk_gib, "backend": choice.backend, "network": mode.name(), "rules": rules, "secrets": secrets });
            let client = Client::connect(&home)?;
            client.post("/api/sandboxes", spec)?;
            install::install(&client, &choice.name, &catalog, &choice.recipes)?;
            println!("{0} is ready; open it with `boxquell shell {0}`", choice.name);
        }
        Cmd::New { name: Some(name), workspace, cpus, ram, backend, allow, secrets, with, disk, mode } => {
            let backend = match backend {
                Some(BackendArg::Krun) => Backend::Krun,
                Some(BackendArg::Qemu) => Backend::Qemu,
                None => Backend::default_for_host(),
            };
            let workspace = workspace.map(std::path::absolute).transpose()?;
            let rules: Vec<String> = allow.iter().map(|host| format!("allow {host}")).collect();
            let spec = json!({ "name": name, "workspace": workspace, "cpus": cpus, "ram_mib": ram, "disk_gib": disk, "network": mode.name(), "backend": backend, "rules": rules, "secrets": secrets });
            let catalog = catalog(&home)?;
            catalog.resolve(&with)?;
            let client = Client::connect(&home)?;
            client.post("/api/sandboxes", spec)?;
            if with.is_empty() {
                println!("created {name}; start it with `boxquell up {name}` or `boxquell shell {name}`");
            } else {
                install::install(&client, &name, &catalog, &with)?;
                println!("{name} is ready; open it with `boxquell shell {name}`");
            }
        }
        Cmd::Up { name } => {
            let info = Client::connect(&home)?.post(&format!("/api/sandboxes/{name}/start"), json!({}))?;
            println!("{name} is running (ssh port {})", info["ssh_port"]);
        }
        Cmd::Down { name } => {
            Client::connect(&home)?.post(&format!("/api/sandboxes/{name}/stop"), json!({}))?;
            println!("{name} stopped");
        }
        Cmd::Rm { name } => {
            Client::connect(&home)?.delete(&format!("/api/sandboxes/{name}"), None)?;
            println!("{name} deleted");
        }
        Cmd::Recipes => {
            for recipe in catalog(&home)?.all() {
                let versions = if recipe.versions.is_empty() { String::new() } else { format!("  [versions: {}]", recipe.versions.join(", ")) };
                println!(
                    "{:<24} {:<9} {:<10} {}{versions}",
                    recipe.id,
                    format!("{:?}", recipe.category).to_lowercase(),
                    recipe.source,
                    recipe.description
                );
            }
        }
        Cmd::Repo { command } => match command {
            RepoCmd::Add { url, name } => {
                let added = repo::add(&home, &url, name.as_deref())?;
                println!(
                    "added {} ({} recipes, revision {}); install with `boxquell add <sandbox> {}:<recipe>`",
                    added.name, added.recipes, added.revision, added.name
                );
                println!("note: its recipes run their own install scripts in the sandbox and open the network destinations they list");
            }
            RepoCmd::Ls => {
                for r in repo::list(&home) {
                    println!("{:<16} {:>3} recipes  {:<9} {}", r.name, r.recipes, r.revision, r.url);
                }
            }
            RepoCmd::Update { name } => {
                let names = match name {
                    Some(name) => vec![name],
                    None => repo::list(&home).into_iter().map(|r| r.name).collect(),
                };
                for name in names {
                    let updated = repo::update(&home, &name)?;
                    println!("{}: {} recipes, revision {}", updated.name, updated.recipes, updated.revision);
                }
            }
            RepoCmd::Rm { name } => {
                repo::remove(&home, &name)?;
                println!("removed {name}");
            }
        },
        Cmd::Add { name, recipes } => install::install(&Client::connect(&home)?, &name, &catalog(&home)?, &recipes)?,
        Cmd::Ui { no_open } => {
            let url = Client::connect(&home)?.ui_url();
            println!("{url}");
            if !no_open {
                let opener = if cfg!(target_os = "macos") {
                    "open"
                } else if cfg!(windows) {
                    "explorer"
                } else {
                    "xdg-open"
                };
                let _ = Command::new(opener).arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
            }
        }
        Cmd::Doctor => {
            if !doctor::run(&home) {
                std::process::exit(1);
            }
        }
        Cmd::Ls => {
            let list = Client::connect(&home)?.get("/api/sandboxes")?;
            println!("{:<20} {:<8} {:<7} {:>4} {:>7}  WORKSPACE", "NAME", "STATE", "BACKEND", "CPUS", "RAM");
            for sandbox in list.as_array().into_iter().flatten() {
                let state = if sandbox["paused"] == true {
                    "paused"
                } else if sandbox["running"] == true {
                    "running"
                } else {
                    "stopped"
                };
                println!(
                    "{:<20} {:<8} {:<7} {:>4} {:>7}  {}",
                    sandbox["name"].as_str().unwrap_or(""),
                    state,
                    sandbox["backend"].as_str().unwrap_or(""),
                    sandbox["cpus"],
                    format!("{}M", sandbox["ram_mib"]),
                    sandbox["workspace"].as_str().unwrap_or("-"),
                );
            }
        }
        Cmd::Shell { name } => ssh_into(&Client::connect(&home)?, &name, None)?,
        Cmd::Run { name, command } => {
            // A login shell loads the sandbox environment (placeholders, CA paths) first.
            let words: Vec<String> = command.iter().map(|word| shell_quote(word)).collect();
            let script = format!("cd /workspace 2>/dev/null; exec {}", words.join(" "));
            ssh_into(&Client::connect(&home)?, &name, Some(format!("bash -lc {}", shell_quote(&script))))?;
        }
        Cmd::Allow { name, host } => {
            Client::connect(&home)?.post(&format!("/api/sandboxes/{name}/rules"), rule("allow", &host))?;
            println!("{name}: allow {host}");
        }
        Cmd::Deny { name, host } => {
            Client::connect(&home)?.post(&format!("/api/sandboxes/{name}/rules"), rule("deny", &host))?;
            println!("{name}: deny {host}");
        }
        Cmd::Unrule { name, rule } => {
            Client::connect(&home)?.delete(&format!("/api/sandboxes/{name}/rules"), Some(json!({ "rule": rule })))?;
            println!("{name}: removed {rule}");
        }
        Cmd::Pause { name } => {
            Client::connect(&home)?.post(&format!("/api/sandboxes/{name}/pause"), json!({}))?;
            println!("{name} paused");
        }
        Cmd::Resume { name } => {
            Client::connect(&home)?.post(&format!("/api/sandboxes/{name}/resume"), json!({}))?;
            println!("{name} resumed");
        }
        Cmd::Global { command } => {
            let client = Client::connect(&home)?;
            let rules = match command {
                GlobalCmd::Allow { host } => client.post("/api/rules", rule("allow", &host))?,
                GlobalCmd::Deny { host } => client.post("/api/rules", rule("deny", &host))?,
                GlobalCmd::Unrule { rule } => client.delete("/api/rules", Some(json!({ "rule": rule })))?,
                GlobalCmd::Ls => client.get("/api/rules")?,
            };
            for rule in rules.as_array().into_iter().flatten() {
                println!("{}", rule.as_str().unwrap_or(""));
            }
        }
        Cmd::Set { name, cpus, ram, disk } => {
            let info = Client::connect(&home)?.patch(&format!("/api/sandboxes/{name}"), json!({ "cpus": cpus, "ram_mib": ram, "disk_gib": disk }))?;
            let hint = if info["running"] == true { " (from its next start)" } else { "" };
            println!("{name}: {} CPUs, {} MiB memory, {} GiB disk{hint}", info["cpus"], info["ram_mib"], info["disk_gib"]);
        }
        Cmd::Port { command } => {
            let client = Client::connect(&home)?;
            let info = match command {
                PortCmd::Add { name, ports } => {
                    let (host, guest) = ports.split_once(':').unwrap_or((&ports, &ports));
                    let (host, guest): (u16, u16) = (host.parse().context("invalid port")?, guest.parse().context("invalid port")?);
                    client.post(&format!("/api/sandboxes/{name}/ports"), json!({ "host": host, "guest": guest }))?
                }
                PortCmd::Rm { name, host } => client.delete(&format!("/api/sandboxes/{name}/ports/{host}"), None)?,
                PortCmd::Ls { name } => client.get(&format!("/api/sandboxes/{name}"))?,
            };
            for port in info["ports"].as_array().into_iter().flatten() {
                println!("http://127.0.0.1:{}  ->  port {} in the sandbox", port["host"], port["guest"]);
            }
        }
        Cmd::ClaudeSettings { name, file, clear } => {
            let client = Client::connect(&home)?;
            if clear {
                client.delete(&format!("/api/sandboxes/{name}/claude-settings"), None)?;
                println!("{name}: Claude settings removed (from its next start)");
            } else {
                let file = file.unwrap_or_else(boxquell_core::claude::host_managed_settings);
                let text = std::fs::read_to_string(&file).with_context(|| format!("cannot read {}", file.display()))?;
                let info = client.put(&format!("/api/sandboxes/{name}/claude-settings"), json!({ "settings": text }))?;
                println!("{name}: uses the Claude settings from {} (from its next start)", file.display());
                for note in info["notes"].as_array().into_iter().flatten() {
                    println!("note: {}", note.as_str().unwrap_or(""));
                }
            }
        }
        Cmd::Mode { name, mode } => {
            Client::connect(&home)?.post(&format!("/api/sandboxes/{name}/mode"), json!({ "mode": mode.name() }))?;
            println!("{name}: {} mode", mode.name());
        }
        Cmd::Policy { name } => {
            let info = Client::connect(&home)?.get(&format!("/api/sandboxes/{name}"))?;
            for rule in info["rules"].as_array().into_iter().flatten() {
                println!("{}", rule.as_str().unwrap_or(""));
            }
            let global = Client::connect(&home)?.get("/api/rules")?;
            for rule in global.as_array().into_iter().flatten() {
                println!("{}   (global)", rule.as_str().unwrap_or(""));
            }
            let blacklist = info["mode"] == "blacklist";
            println!("{} everything else ({} mode)", if blacklist { "allow" } else { "deny" }, if blacklist { "blacklist" } else { "whitelist" });
            if blacklist {
                println!("private and local addresses still need a rule naming them");
            }
        }
        Cmd::Logs { name, follow, blocked, lines } => {
            let client = Client::connect(&home)?;
            let mut query = format!("limit={lines}&blocked_only={blocked}");
            if let Some(name) = &name {
                query.push_str(&format!("&sandbox={name}"));
            }
            let recent = client.get(&format!("/api/events?{query}"))?;
            recent.as_array().into_iter().flatten().for_each(print_event);
            if follow {
                client.follow(&query, print_event)?;
            }
        }
        Cmd::Secret { command } => {
            let client = Client::connect(&home)?;
            match command {
                SecretCmd::Set { name, sandbox, hosts, env, placeholder } => {
                    if hosts.is_empty() && SecretDef::preset(&name).is_none() {
                        bail!("{name:?} is not a known service ({}); pass --host for your own secrets", SecretDef::PRESETS.join(", "));
                    }
                    let value = if std::io::stdin().is_terminal() {
                        rpassword::prompt_password(format!("Value for {name}: "))?
                    } else {
                        std::io::read_to_string(std::io::stdin())?
                    };
                    let mut body = json!({ "value": value.trim() });
                    if !hosts.is_empty() {
                        body["hosts"] = hosts.into();
                    }
                    if !env.is_empty() {
                        body["env"] = env.into();
                    }
                    if let Some(placeholder) = placeholder {
                        body["placeholder"] = placeholder.into();
                    }
                    match sandbox {
                        Some(sandbox) => {
                            client.put(&format!("/api/sandboxes/{sandbox}/secrets/{name}"), body)?;
                            println!("stored {name} for {sandbox} only; a running sandbox gets its placeholder variable after a restart");
                        }
                        None => {
                            let saved = client.put(&format!("/api/secrets/{name}"), body)?;
                            println!("stored {name}: sent only to {}, visible in sandboxes as {}", saved["hosts"], saved["env"]);
                        }
                    }
                }
                SecretCmd::Ls => {
                    for secret in client.get("/api/secrets")?.as_array().into_iter().flatten() {
                        println!("{:<16} hosts={} env={}", secret["name"].as_str().unwrap_or(""), secret["hosts"], secret["env"]);
                    }
                }
                SecretCmd::Rm { name, sandbox: Some(sandbox) } => {
                    client.delete(&format!("/api/sandboxes/{sandbox}/secrets/{name}"), None)?;
                    println!("removed {name} from {sandbox}");
                }
                SecretCmd::Rm { name, sandbox: None } => {
                    let result = client.delete(&format!("/api/secrets/{name}"), None)?;
                    println!("{}", if result["removed"] == true { format!("removed {name}") } else { format!("no secret named {name}") });
                }
                SecretCmd::Attach { sandbox, name } => {
                    let info = client.post(&format!("/api/sandboxes/{sandbox}/secrets"), json!({ "secret": name }))?;
                    let hint = if info["running"] == true { "; restart the sandbox to get its placeholder variable" } else { "" };
                    println!("{sandbox} can now use {name}{hint}");
                }
            }
        }
    }
    Ok(())
}
