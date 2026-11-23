//! How the host reaches a sandbox: the system's ssh client through the
//! loopback port the daemon forwards into the guest.

use std::ffi::OsString;
use std::path::Path;

pub const GUEST_USER: &str = "dev";

/// Arguments for `ssh` up to and including the destination.
pub fn base_args(key: &Path, port: u16) -> Vec<OsString> {
    // Guest host keys change whenever a sandbox is recreated and the port is
    // only reachable from this machine, so host key checking adds nothing.
    let known_hosts = if cfg!(windows) { "UserKnownHostsFile=NUL" } else { "UserKnownHostsFile=/dev/null" };
    let mut args: Vec<OsString> = vec!["-i".into(), key.into(), "-p".into(), port.to_string().into()];
    for option in ["StrictHostKeyChecking=no", known_hosts, "LogLevel=ERROR", "IdentitiesOnly=yes", "BatchMode=yes"] {
        args.push("-o".into());
        args.push(option.into());
    }
    args.push(format!("{GUEST_USER}@127.0.0.1").into());
    args
}

/// The name under which `ssh`, editors and IDEs reach a sandbox: `ssh boxquell-<name>`.
pub fn host_alias(sandbox: &str) -> String {
    format!("boxquell-{sandbox}")
}

fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// One `Host` block per sandbox for an OpenSSH client configuration.
/// The connection goes through `boxquell ssh-proxy`, which starts the sandbox
/// when needed, so the entry stays valid although the forwarded port changes.
pub fn client_config(home: &boxquell_core::Home, exe: &Path, sandboxes: &[String]) -> String {
    // A data directory or libkrun chosen through the environment has to reach the proxy too.
    let env: String = ["BOXQUELL_HOME", "BOXQUELL_KRUN_LIB"]
        .iter()
        .filter_map(|var| std::env::var(var).ok().filter(|v| !v.is_empty() && cfg!(unix)).map(|value| format!("{var}=\"{value}\" ")))
        .collect();
    let env = if env.is_empty() { env } else { format!("env {env}") };
    let mut text = String::from("# Written by `boxquell ssh-config`. Changes are overwritten.\n");
    for name in sandboxes {
        text += &format!(
            "\nHost {alias}\n    HostName 127.0.0.1\n    User {GUEST_USER}\n    IdentityFile {key}\n    IdentitiesOnly yes\n    StrictHostKeyChecking no\n    UserKnownHostsFile {null}\n    LogLevel ERROR\n    ProxyCommand {env}{exe} ssh-proxy {name}\n",
            alias = host_alias(name),
            key = quoted(&home.sandbox(name).join("id_ed25519")),
            null = if cfg!(windows) { "NUL" } else { "/dev/null" },
            exe = quoted(exe),
        );
    }
    text
}

/// `~/.ssh/boxquell_config`, the file `boxquell ssh-config --install` maintains.
pub fn config_file() -> Option<std::path::PathBuf> {
    Some(std::env::home_dir()?.join(".ssh").join("boxquell_config"))
}

/// Write the configuration and make `~/.ssh/config` include it. Returns the file written.
pub fn install_config(text: &str) -> anyhow::Result<std::path::PathBuf> {
    use anyhow::Context;
    let file = config_file().context("cannot find your home directory")?;
    let dir = file.parent().expect("has a parent").to_path_buf();
    std::fs::create_dir_all(&dir)?;
    boxquell_core::write_private(&file, text.as_bytes())?;
    let main = dir.join("config");
    let existing = std::fs::read_to_string(&main).unwrap_or_default();
    if !existing.lines().any(|line| line.trim().eq_ignore_ascii_case("include boxquell_config")) {
        // `Include` has to come before the first `Host` block to apply to all hosts.
        boxquell_core::write_private(&main, format!("Include boxquell_config\n\n{existing}").as_bytes())?;
    }
    Ok(file)
}

/// Connect standard input and output to the sandbox's ssh port. Used as `ProxyCommand`.
pub fn proxy(port: u16) -> anyhow::Result<()> {
    use std::io::Write;
    let stream = std::net::TcpStream::connect(("127.0.0.1", port))?;
    stream.set_nodelay(true)?;
    let mut to_guest = stream.try_clone()?;
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut std::io::stdin().lock(), &mut to_guest);
        let _ = to_guest.shutdown(std::net::Shutdown::Write);
    });
    // Standard output buffers until a line ends, which never happens in an
    // encrypted stream: pass every chunk on at once.
    use std::io::Read;
    let mut from_guest = stream;
    let mut stdout = std::io::stdout().lock();
    let mut buf = [0u8; 16 * 1024];
    loop {
        let n = from_guest.read(&mut buf)?;
        if n == 0 {
            break;
        }
        stdout.write_all(&buf[..n])?;
        stdout.flush()?;
    }
    Ok(())
}
