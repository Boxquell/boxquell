//! boxquell: run coding agents in microVMs whose network access you control.

mod api;
mod client;
mod daemon;
mod doctor;
mod install;
mod keyagent;
mod mcp;
mod notify;
mod pull;
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
        /// Operating system, see `boxquell os ls`
        #[arg(long, default_value = "debian")]
        os: String,
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
        /// Start from a template saved with `boxquell template save`: its disk, system and software
        #[arg(long, value_name = "NAME")]
        template: Option<String>,
    },
    /// Save the state of a sandbox's disk and come back to it later
    Snapshot {
        #[command(subcommand)]
        command: SnapshotCmd,
    },
    /// Sandboxes saved as a starting point for new ones (`boxquell new --template`)
    Template {
        #[command(subcommand)]
        command: TemplateCmd,
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
    /// Let `ssh`, VS Code and JetBrains reach sandboxes as `ssh boxquell-<name>`
    SshConfig {
        /// Write ~/.ssh/boxquell_config and include it from ~/.ssh/config, instead of printing
        #[arg(long)]
        install: bool,
    },
    /// Internal: carry an ssh connection into a sandbox
    #[command(hide = true)]
    SshProxy { name: String },
    /// Check that this host can run sandboxes
    Doctor,
    /// Print a completion script for your shell, e.g. `boxquell completions fish > ~/.config/fish/completions/boxquell.fish`
    Completions {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
    /// Open the web UI for all sandboxes in the browser
    Ui {
        /// Only print the address
        #[arg(long)]
        no_open: bool,
    },
    /// List what can be installed into a sandbox (agents, languages, tools)
    Recipes,
    /// Operating systems for sandboxes: list them, or build one's disk image
    Os {
        #[command(subcommand)]
        command: OsCmd,
    },
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
        /// Run the install again although the software is there, e.g. to update it
        #[arg(long)]
        again: bool,
    },
    /// Remove installed software from a sandbox, e.g. `boxquell remove box python`
    Remove {
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
    /// Instructions (CLAUDE.md / AGENTS.md), skills and MCP servers for the agents, for all sandboxes or one
    Agent {
        #[command(subcommand)]
        command: AgentCmd,
    },
    /// Emergency stop: block every connection of every sandbox at once, and end open ones
    Lockdown {
        /// `on`, `off`, or nothing to show the state
        #[arg(value_parser = ["on", "off"])]
        state: Option<String>,
    },
    /// Preferences, e.g. `boxquell config notify-blocked off`
    Config {
        /// `notify-blocked`, `toast-blocked`, `browser-ui` (on|off), `listen` (addresses separated by commas, or `off`), `listen-port`
        #[arg(value_parser = ["notify-blocked", "toast-blocked", "browser-ui", "listen", "listen-port"])]
        key: Option<String>,
        value: Option<String>,
    },
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
        /// Directory in the sandbox where shells and agents start, e.g. /workspace/backend (`default` for /workspace)
        #[arg(long)]
        workdir: Option<String>,
        /// Start this with the sandbox: an installed agent's name such as `orca`, or a command (repeatable; `none` clears)
        #[arg(long = "autostart", value_name = "AGENT|COMMAND")]
        autostart: Vec<String>,
        /// Environment variable for sessions in the sandbox, `KEY=value`; `KEY=` removes it (repeatable)
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
    },
    /// Serve the Model Context Protocol on standard input and output, so an assistant on this computer can work with sandboxes
    Mcp {
        /// Offer only tools that look: no create, start, stop, run or install
        #[arg(long)]
        read_only: bool,
        /// Also offer tools that allow and deny network destinations
        #[arg(long)]
        allow_network_rules: bool,
        /// Also offer the tool that deletes a sandbox
        #[arg(long)]
        allow_delete: bool,
    },
    /// SSH keys of this computer a sandbox may use, e.g. for git over SSH; the keys stay on this computer
    SshKeys {
        /// The sandbox; without it the choice applies to all sandboxes
        name: Option<String>,
        /// Share every key loaded in this computer's SSH agent
        #[arg(long, value_parser = ["on", "off"])]
        agent: Option<String>,
        /// Share this private key file, e.g. ~/.ssh/id_ed25519 (repeatable)
        #[arg(long, value_name = "FILE")]
        add: Vec<PathBuf>,
        /// Stop sharing this key file (repeatable)
        #[arg(long, value_name = "FILE")]
        rm: Vec<PathBuf>,
    },
    /// Share more directories of this computer with a sandbox (libkrun backend)
    Share {
        #[command(subcommand)]
        command: ShareCmd,
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
enum AgentCmd {
    /// Show what is configured and where it is stored
    Show {
        /// For this sandbox only (default: for all sandboxes)
        #[arg(long)]
        sandbox: Option<String>,
    },
    /// Set instructions and MCP servers from files; an empty file removes them
    Set {
        /// Markdown file with instructions; agents get it as CLAUDE.md / AGENTS.md
        #[arg(long)]
        instructions: Option<PathBuf>,
        /// JSON file with MCP servers: { "name": { "command": "...", "args": [...] } }
        #[arg(long)]
        mcp: Option<PathBuf>,
        #[arg(long)]
        sandbox: Option<String>,
    },
    /// Add a skill: a directory with a SKILL.md, e.g. ~/.claude/skills/my-skill
    SkillAdd {
        dir: PathBuf,
        #[arg(long)]
        sandbox: Option<String>,
    },
    /// Remove a skill
    SkillRm {
        name: String,
        #[arg(long)]
        sandbox: Option<String>,
    },
}

#[derive(Subcommand)]
enum OsCmd {
    /// List the systems on offer and whether their image is built
    Ls,
    /// Get a system ready: download its ready-made image, or build it here if there is none
    Install { id: String },
    /// Download the ready-made image of an official system
    Pull { id: String },
    /// Build the disk image of a system on this computer (needs Docker); takes a few minutes
    Build {
        id: String,
        /// Size of the image in GiB; sandboxes grow their copy to their own disk size
        #[arg(long, default_value_t = 4)]
        size: u32,
    },
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
enum SnapshotCmd {
    /// `boxquell snapshot take box before the big refactoring`
    Take {
        name: String,
        /// A few words to recognise it by
        label: Vec<String>,
    },
    Ls {
        name: String,
    },
    /// Put the sandbox's disk back to a snapshot; what happened since is gone
    Restore {
        name: String,
        id: String,
    },
    Rm {
        name: String,
        id: String,
    },
}

#[derive(Subcommand)]
enum TemplateCmd {
    /// `boxquell template save box node-project`
    Save {
        sandbox: String,
        name: String,
        description: Vec<String>,
    },
    Ls,
    Rm {
        name: String,
    },
}

#[derive(Subcommand)]
enum ShareCmd {
    /// `boxquell share add box ~/data /mnt/data --ro`
    Add {
        name: String,
        /// Directory on this computer
        host: PathBuf,
        /// Where it appears in the sandbox
        guest: String,
        /// Mount it read-only (guards against accidents, not against root in the sandbox)
        #[arg(long)]
        ro: bool,
    },
    /// Stop sharing the directory mounted at this path in the sandbox
    Rm {
        name: String,
        guest: String,
    },
    Ls {
        name: String,
    },
}

#[derive(Subcommand)]
enum PortCmd {
    /// `boxquell port add box 3000` or `boxquell port add box 8080:3000` (this computer:sandbox)
    Add {
        name: String,
        ports: String,
        /// Address of this computer to listen on, for other devices (default: 127.0.0.1, this computer only)
        #[arg(long)]
        bind: Option<std::net::IpAddr>,
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

/// The start command of the agent `word` names, if a recipe of that name is installed in the sandbox.
fn installed_agent_command(home: &Home, client: &Client, sandbox: &str, word: &str) -> Option<String> {
    let info = client.get(&format!("/api/sandboxes/{sandbox}")).ok()?;
    let catalog = Catalog::of(home).ok()?;
    info["recipes"].as_array()?.iter().filter_map(|entry| entry.as_str()).find_map(|entry| {
        let id = entry.split('@').next()?;
        let short = id.rsplit(':').next()?;
        (short == word).then(|| catalog.get(id)?.command.clone()).flatten()
    })
}

/// A sandbox can be created on a system whose image does not exist yet: build it first.
fn build_system_if_missing(home: &Home, os: &str) -> Result<()> {
    if boxquell_core::os::image(home, os).exists() {
        return Ok(());
    }
    for problem in repo::ensure_official(home) {
        eprintln!("warning: {problem}");
    }
    println!("the system {os} is not on this computer yet; getting it first");
    pull::install(home, os).map(drop)
}

fn ssh_config(home: &Home, client: &Client) -> Result<String> {
    let names: Vec<String> =
        client.get("/api/sandboxes")?.as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_string)).collect();
    Ok(ssh::client_config(home, &std::env::current_exe()?, &names))
}

/// Keep an installed ssh configuration in step with the sandboxes that exist.
fn refresh_ssh_config(home: &Home, client: &Client) {
    if ssh::config_file().is_some_and(|file| file.exists()) {
        if let Ok(text) = ssh_config(home, client) {
            let _ = ssh::install_config(&text);
        }
    }
}

/// What a repository offers, e.g. `10 recipes` or `3 systems`.
fn repo_contents(repo: &repo::Repo) -> String {
    let count = |n: usize, one: &str| format!("{n} {one}{}", if n == 1 { "" } else { "s" });
    match (repo.recipes, repo.systems) {
        (recipes, 0) => count(recipes, "recipe"),
        (0, systems) => count(systems, "system"),
        (recipes, systems) => format!("{}, {}", count(recipes, "recipe"), count(systems, "system")),
    }
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
    for warning in info["warnings"].as_array().into_iter().flatten() {
        eprintln!("warning: {}", warning.as_str().unwrap_or(""));
    }
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
    if let Cmd::Completions { shell } = &cli.command {
        use clap::CommandFactory;
        clap_complete::generate(*shell, &mut Cli::command(), "boxquell", &mut std::io::stdout());
        return Ok(());
    }
    let home = Home::locate()?;
    match cli.command {
        Cmd::Completions { .. } => unreachable!("handled above"),
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
        Cmd::New { name: None, allow, secrets, mode, os, .. } => {
            if !std::io::stdin().is_terminal() {
                bail!("give the sandbox a name: `boxquell new <name>` (the interactive form needs a terminal)");
            }
            let catalog = catalog(&home)?;
            let Some(choice) = wizard::run(&catalog)? else { return Ok(()) };
            let rules: Vec<String> = allow.iter().map(|host| format!("allow {host}")).collect();
            let spec = json!({ "name": choice.name, "workspace": choice.workspace, "cpus": choice.cpus, "ram_mib": choice.ram_mib,
                "disk_gib": choice.disk_gib, "backend": choice.backend, "os": os, "network": mode.name(), "rules": rules, "secrets": secrets });
            let client = Client::connect(&home)?;
            build_system_if_missing(&home, &os)?;
            client.post("/api/sandboxes", spec)?;
            install::install(&client, &choice.name, &catalog, &choice.recipes, false)?;
            println!("{0} is ready; open it with `boxquell shell {0}`", choice.name);
        }
        Cmd::New { name: Some(name), workspace, cpus, ram, backend, allow, secrets, with, disk, mode, os, template } => {
            let backend = match backend {
                Some(BackendArg::Krun) => Backend::Krun,
                Some(BackendArg::Qemu) => Backend::Qemu,
                None => Backend::default_for_host(),
            };
            let workspace = workspace.map(std::path::absolute).transpose()?;
            let rules: Vec<String> = allow.iter().map(|host| format!("allow {host}")).collect();
            let spec = json!({ "name": name, "workspace": workspace, "cpus": cpus, "ram_mib": ram, "disk_gib": disk, "os": os, "network": mode.name(), "backend": backend, "rules": rules, "secrets": secrets, "template": template });
            let catalog = catalog(&home)?;
            catalog.resolve(&with)?;
            let client = Client::connect(&home)?;
            build_system_if_missing(&home, &os)?;
            client.post("/api/sandboxes", spec)?;
            refresh_ssh_config(&home, &client);
            if with.is_empty() {
                println!("created {name}; start it with `boxquell up {name}` or `boxquell shell {name}`");
            } else {
                install::install(&client, &name, &catalog, &with, false)?;
                println!("{name} is ready; open it with `boxquell shell {name}`");
            }
        }
        Cmd::Snapshot { command } => {
            let client = Client::connect(&home)?;
            let (name, info) = match command {
                SnapshotCmd::Take { name, label } => {
                    println!("{name}: saving its disk (a running sandbox is stopped for it and started again)…");
                    let info = client.post(&format!("/api/sandboxes/{name}/snapshots"), json!({ "label": label.join(" ") }))?;
                    (name, info)
                }
                SnapshotCmd::Restore { name, id } => {
                    let info = client.post(&format!("/api/sandboxes/{name}/snapshots/{id}"), json!({}))?;
                    println!("{name}: back at snapshot {id}");
                    (name, info)
                }
                SnapshotCmd::Rm { name, id } => {
                    let info = client.delete(&format!("/api/sandboxes/{name}/snapshots/{id}"), None)?;
                    (name, info)
                }
                SnapshotCmd::Ls { name } => {
                    let info = client.get(&format!("/api/sandboxes/{name}"))?;
                    (name, info)
                }
            };
            let snapshots = info["snapshots"].as_array().cloned().unwrap_or_default();
            if snapshots.is_empty() {
                println!("{name} has no snapshots; take one with `boxquell snapshot take {name}`");
            }
            for snapshot in snapshots {
                println!("{:<14} {}", snapshot["id"].as_str().unwrap_or(""), snapshot["label"].as_str().unwrap_or(""));
            }
        }
        Cmd::Template { command } => {
            let client = Client::connect(&home)?;
            match command {
                TemplateCmd::Save { sandbox, name, description } => {
                    println!("saving {sandbox} as the template {name} (a running sandbox is stopped for it and started again)…");
                    client.post(&format!("/api/sandboxes/{sandbox}/template"), json!({ "name": name, "description": description.join(" ") }))?;
                    println!("start from it with `boxquell new <name> --template {name}`");
                }
                TemplateCmd::Rm { name } => {
                    client.delete(&format!("/api/templates/{name}"), None)?;
                }
                TemplateCmd::Ls => {}
            }
            for template in client.get("/api/templates")?.as_array().into_iter().flatten() {
                let software: Vec<&str> = template["recipes"].as_array().into_iter().flatten().filter_map(|r| r.as_str()).collect();
                println!("{:<16} {:<10} {}", template["name"].as_str().unwrap_or(""), template["os"].as_str().unwrap_or(""), software.join(", "));
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
            let client = Client::connect(&home)?;
            client.delete(&format!("/api/sandboxes/{name}"), None)?;
            println!("{name} deleted");
            refresh_ssh_config(&home, &client);
        }
        Cmd::SshConfig { install } => {
            let client = Client::connect(&home)?;
            let text = ssh_config(&home, &client)?;
            if install {
                let file = ssh::install_config(&text)?;
                println!("wrote {}; connect with `ssh boxquell-<name>`", file.display());
            } else {
                print!("{text}");
            }
        }
        Cmd::SshProxy { name } => {
            let info = Client::connect(&home)?.post(&format!("/api/sandboxes/{name}/start"), json!({}))?;
            ssh::proxy(info["ssh_port"].as_u64().context("daemon reported no ssh port")? as u16)?;
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
                println!("added {} ({}, revision {})", added.name, repo_contents(&added), added.revision);
                if added.recipes > 0 {
                    println!("install its recipes with `boxquell add <sandbox> {}:<recipe>`", added.name);
                }
                if added.systems > 0 {
                    println!("build its systems with `boxquell os build <id>`, see `boxquell os ls`");
                }
                println!("note: its recipes run their own install scripts in the sandbox and open the network destinations they list");
            }
            RepoCmd::Ls => {
                for r in repo::list(&home) {
                    println!("{:<16} {:<22} {:<9} {}", r.name, repo_contents(&r), r.revision, r.url);
                }
            }
            RepoCmd::Update { name } => {
                let names = match name {
                    Some(name) => vec![name],
                    None => repo::list(&home).into_iter().map(|r| r.name).collect(),
                };
                for name in names {
                    let updated = repo::update(&home, &name)?;
                    println!("{}: {}, revision {}", updated.name, repo_contents(&updated), updated.revision);
                }
            }
            RepoCmd::Rm { name } => {
                repo::remove(&home, &name)?;
                println!("removed {name}");
            }
        },
        Cmd::Add { name, recipes, again } => install::install(&Client::connect(&home)?, &name, &catalog(&home)?, &recipes, again)?,
        Cmd::Remove { name, recipes } => install::uninstall(&Client::connect(&home)?, &name, &catalog(&home)?, &recipes)?,
        Cmd::Ui { no_open } => {
            let client = Client::connect(&home)?;
            let url = client.ui_url();
            if no_open {
                println!("{url}");
            } else if client.get("/api/settings")?["browser_ui"] == true {
                println!("{url}");
                let opener = if cfg!(target_os = "macos") {
                    "open"
                } else if cfg!(windows) {
                    "explorer"
                } else {
                    "xdg-open"
                };
                let _ = Command::new(opener).arg(&url).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
            } else {
                // The desktop app is the interface unless the browser one was switched on.
                let name = if cfg!(windows) { "boxquell-desktop.exe" } else { "boxquell-desktop" };
                let beside = std::env::current_exe().ok().and_then(|exe| Some(exe.parent()?.join(name))).filter(|path| path.is_file());
                let app = std::env::var_os("BOXQUELL_DESKTOP").map(PathBuf::from).or(beside).unwrap_or_else(|| PathBuf::from(name));
                let started = Command::new(&app)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
                if started.is_err() {
                    bail!("the Boxquell app was not found ({}). Install it, or allow the interface in web browsers with `boxquell config browser-ui on`", app.display());
                }
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
            let client = Client::connect(&home)?;
            // The name of an installed agent starts it the way its recipe says, e.g. `boxquell run box orca`.
            let agent = match command.as_slice() {
                [word] => installed_agent_command(&home, &client, &name, word),
                _ => None,
            };
            let script = match agent {
                // Recipe commands are shell text: they may refer to variables of the sandbox.
                Some(recipe_command) => format!("exec {recipe_command}"),
                None => format!("exec {}", command.iter().map(|word| shell_quote(word)).collect::<Vec<_>>().join(" ")),
            };
            ssh_into(&client, &name, Some(format!("bash -lc {}", shell_quote(&script))))?;
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
        Cmd::Agent { command } => {
            let client = Client::connect(&home)?;
            let base = |sandbox: &Option<String>| match sandbox {
                Some(name) => format!("/api/sandboxes/{name}/agent-config"),
                None => "/api/agent-config".to_string(),
            };
            let config = match command {
                AgentCmd::Show { sandbox } => client.get(&base(&sandbox))?,
                AgentCmd::Set { instructions, mcp, sandbox } => {
                    let current = client.get(&base(&sandbox))?;
                    let read = |file: &Option<PathBuf>, key: &str| -> Result<String> {
                        match file {
                            Some(file) => std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display())),
                            None => Ok(current[key].as_str().unwrap_or_default().to_string()),
                        }
                    };
                    client.put(&base(&sandbox), json!({ "instructions": read(&instructions, "instructions")?, "mcp": read(&mcp, "mcp")? }))?
                }
                AgentCmd::SkillAdd { dir, sandbox } => {
                    client.post(&format!("{}/skills", base(&sandbox)), json!({ "path": std::path::absolute(dir)? }))?
                }
                AgentCmd::SkillRm { name, sandbox } => client.delete(&format!("{}/skills/{name}", base(&sandbox)), None)?,
            };
            let lines = config["instructions"].as_str().unwrap_or_default().lines().count();
            let servers: Vec<String> = serde_json::from_str::<Value>(config["mcp"].as_str().unwrap_or("{}"))
                .ok()
                .and_then(|v| v.as_object().map(|m| m.keys().cloned().collect()))
                .unwrap_or_default();
            let skills: Vec<&str> = config["skills"].as_array().into_iter().flatten().filter_map(|s| s.as_str()).collect();
            let none = |list: String| if list.is_empty() { "none".to_string() } else { list };
            println!("instructions  {}", if lines == 0 { "none".to_string() } else { format!("{lines} lines") });
            println!("mcp servers   {}", none(servers.join(", ")));
            println!("skills        {}", none(skills.join(", ")));
            println!("stored in     {}", config["path"].as_str().unwrap_or_default());
            println!("A sandbox picks changes up the next time it starts.");
        }
        Cmd::Os { command } => {
            for problem in repo::ensure_official(&home) {
                eprintln!("warning: {problem}");
            }
            match command {
                OsCmd::Ls => {
                    for def in boxquell_core::os::definitions(&home) {
                        println!(
                            "{:<10} {:<10} {:<9} {:<16} {}",
                            def.id,
                            if def.built { "built" } else { "not built" },
                            def.family,
                            def.name,
                            def.description
                        );
                    }
                }
                OsCmd::Install { id } => {
                    let image = pull::install(&home, &id)?;
                    println!("{id} is ready ({}); create a sandbox with `boxquell new <name> --os {id}`", image.display());
                }
                OsCmd::Pull { id } => {
                    let image = pull::pull(&home, &id)?;
                    println!("downloaded {}; create a sandbox with `boxquell new <name> --os {id}`", image.display());
                }
                OsCmd::Build { id, size } => {
                    println!("building {id}; this takes a few minutes");
                    let image = boxquell_core::os::build(&home, &id, size)?;
                    println!("built {}; create a sandbox with `boxquell new <name> --os {id}`", image.display());
                }
            }
        }
        Cmd::Lockdown { state } => {
            let client = Client::connect(&home)?;
            let now = match state {
                Some(state) => client.post("/api/lockdown", json!({ "on": state == "on" }))?,
                None => client.get("/api/lockdown")?,
            };
            if now["on"] == true {
                println!("LOCKDOWN is on: no sandbox can reach the network. Lift it with `boxquell lockdown off`.");
            } else {
                println!("lockdown is off: sandboxes follow their network rules");
            }
        }
        Cmd::Config { key, value } => {
            let client = Client::connect(&home)?;
            let mut settings = client.get("/api/settings")?;
            if let (Some(key), Some(value)) = (&key, &value) {
                settings[key.replace('-', "_")] = match key.as_str() {
                    "listen" if value == "off" => json!([]),
                    "listen" => value
                        .split(',')
                        .map(|a| a.trim().parse::<std::net::IpAddr>().map(|a| json!(a)))
                        .collect::<Result<Vec<_>, _>>()
                        .context("invalid address")?
                        .into(),
                    "listen-port" => value.parse::<u16>().context("invalid port")?.into(),
                    _ if value == "on" || value == "off" => (value == "on").into(),
                    _ => bail!("{key} is either on or off"),
                };
                settings = client.put("/api/settings", settings)?;
                if key.starts_with("listen") {
                    println!("note: applies after the daemon restarts (`boxquell daemon --stop`); traffic on these addresses is not encrypted");
                }
            }
            for key in ["notify_blocked", "toast_blocked", "browser_ui"] {
                println!("{:<16} {}", key.replace('_', "-"), if settings[key] == true { "on" } else { "off" });
            }
            let listen: Vec<&str> = settings["listen"].as_array().into_iter().flatten().filter_map(|a| a.as_str()).collect();
            println!("{:<16} {}", "listen", if listen.is_empty() { "off (this computer only)".to_string() } else { listen.join(", ") });
            println!("{:<16} {}", "listen-port", settings["listen_port"]);
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
        Cmd::Set { name, cpus, ram, disk, workdir, env, autostart } => {
            let autostart = (!autostart.is_empty()).then_some(autostart);
            let env: serde_json::Map<String, Value> = env
                .iter()
                .map(|pair| {
                    pair.split_once('=')
                        .map(|(key, value)| (key.to_string(), Value::from(value)))
                        .with_context(|| format!("expected KEY=value, got {pair:?}"))
                })
                .collect::<Result<_>>()?;
            let info = Client::connect(&home)?.patch(
                &format!("/api/sandboxes/{name}"),
                json!({ "cpus": cpus, "ram_mib": ram, "disk_gib": disk, "workdir": workdir, "env": env, "autostart": autostart }),
            )?;
            let hint = if info["running"] == true { " (from its next start)" } else { "" };
            println!("{name}: {} CPUs, {} MiB memory, {} GiB disk{hint}", info["cpus"], info["ram_mib"], info["disk_gib"]);
            for entry in info["autostart"].as_array().into_iter().flatten() {
                println!("  starts with the sandbox: {}", entry.as_str().unwrap_or_default());
            }
            for (key, value) in info["env"].as_object().into_iter().flatten() {
                println!("  {key}={}", value.as_str().unwrap_or_default());
            }
        }
        Cmd::Mcp { read_only, allow_network_rules, allow_delete } => {
            mcp::serve(mcp::Access { read_only, network_rules: allow_network_rules, delete: allow_delete })?;
        }
        Cmd::SshKeys { name, agent, add, rm } => {
            let client = Client::connect(&home)?;
            let path = match &name {
                Some(name) => format!("/api/sandboxes/{name}/ssh-keys"),
                None => "/api/ssh-keys".to_string(),
            };
            let mut info = client.get(&path)?;
            if agent.is_some() || !add.is_empty() || !rm.is_empty() {
                let mut files: Vec<PathBuf> =
                    info["files"].as_array().into_iter().flatten().filter_map(|file| file.as_str().map(PathBuf::from)).collect();
                for file in &rm {
                    let file = std::path::absolute(file)?;
                    files.retain(|kept| *kept != file);
                }
                for file in &add {
                    let file = std::path::absolute(file)?;
                    if !files.contains(&file) {
                        files.push(file);
                    }
                }
                let agent = agent.as_deref().map_or(info["agent"] == true, |state| state == "on");
                info = client.put(&path, json!({ "agent": agent, "files": files }))?;
            }
            let scope = name.as_deref().map_or("all sandboxes".to_string(), |name| format!("sandbox {name}"));
            println!("SSH keys for {scope}:");
            println!("  keys in this computer's SSH agent: {}", if info["agent"] == true { "shared" } else { "not shared" });
            for file in info["files"].as_array().into_iter().flatten() {
                println!("  key file: {}", file.as_str().unwrap_or_default());
            }
            if name.is_some() {
                let global = &info["global"];
                if global["agent"] == true {
                    println!("  for all sandboxes: the SSH agent");
                }
                for file in global["files"].as_array().into_iter().flatten() {
                    println!("  for all sandboxes: {}", file.as_str().unwrap_or_default());
                }
            }
            for problem in info["problems"].as_array().into_iter().flatten() {
                println!("warning: {}", problem.as_str().unwrap_or_default());
            }
            if info["agent"] != true && info["files"].as_array().is_none_or(|files| files.is_empty()) {
                println!(
                    "Share one with --add ~/.ssh/id_ed25519 or --agent on. The keys stay on this computer; the sandbox can only sign with them."
                );
                println!("Git over SSH also needs its port allowed, e.g. `boxquell allow <sandbox> github.com:22`.");
            }
        }
        Cmd::Share { command } => {
            let client = Client::connect(&home)?;
            let info = match command {
                ShareCmd::Add { name, host, guest, ro } => client
                    .post(&format!("/api/sandboxes/{name}/shares"), json!({ "host": std::path::absolute(host)?, "guest": guest, "read_only": ro }))?,
                ShareCmd::Rm { name, guest } => client.delete(&format!("/api/sandboxes/{name}/shares"), Some(json!({ "guest": guest })))?,
                ShareCmd::Ls { name } => client.get(&format!("/api/sandboxes/{name}"))?,
            };
            for share in info["shares"].as_array().into_iter().flatten() {
                println!(
                    "{}  ->  {}{}",
                    share["host"].as_str().unwrap_or(""),
                    share["guest"].as_str().unwrap_or(""),
                    if share["read_only"] == true { "  (read-only)" } else { "" }
                );
            }
            if info["running"] == true {
                println!("note: changes apply from the sandbox's next start");
            }
        }
        Cmd::Port { command } => {
            let client = Client::connect(&home)?;
            let info = match command {
                PortCmd::Add { name, ports, bind } => {
                    let (host, guest) = ports.split_once(':').unwrap_or((&ports, &ports));
                    let (host, guest): (u16, u16) = (host.parse().context("invalid port")?, guest.parse().context("invalid port")?);
                    client.post(&format!("/api/sandboxes/{name}/ports"), json!({ "host": host, "guest": guest, "bind": bind }))?
                }
                PortCmd::Rm { name, host } => client.delete(&format!("/api/sandboxes/{name}/ports/{host}"), None)?,
                PortCmd::Ls { name } => client.get(&format!("/api/sandboxes/{name}"))?,
            };
            for port in info["ports"].as_array().into_iter().flatten() {
                println!("http://{}:{}  ->  port {} in the sandbox", port["bind"].as_str().unwrap_or("127.0.0.1"), port["host"], port["guest"]);
            }
        }
        Cmd::ClaudeSettings { name, file, clear } => {
            let client = Client::connect(&home)?;
            if clear {
                client.delete(&format!("/api/sandboxes/{name}/claude-settings"), None)?;
                println!("{name}: Claude settings removed (from its next start)");
            } else {
                // Without a file: this computer's managed settings, with the drop-in files of managed-settings.d.
                let (text, file) = match file {
                    Some(file) => (std::fs::read_to_string(&file).with_context(|| format!("cannot read {}", file.display()))?, file),
                    None => (boxquell_core::claude::host_managed_text()?, boxquell_core::claude::host_managed_settings()),
                };
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
            // What the system and installed software open by themselves.
            for group in info["auto_rules"].as_array().into_iter().flatten() {
                for entry in group["hosts"].as_array().into_iter().flatten() {
                    let state = if entry["enabled"] == true { "allow" } else { "off  " };
                    println!("{state} {}   (opened by {})", entry["host"].as_str().unwrap_or(""), group["name"].as_str().unwrap_or(""));
                }
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
