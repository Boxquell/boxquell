//! SSH keys for git in a sandbox, without handing the keys over.
//!
//! The private keys stay on this computer. The sandbox gets a socket
//! (`SSH_AUTH_SOCK`) that leads to an SSH agent here, which signs for it:
//! either the user's own agent or one started just for the chosen key files.

use std::path::{Path, PathBuf};

use boxquell_core::spec::SshKeys;
use serde_json::{json, Value};

/// Where the socket appears in the guest; `boxquell-init` creates the directory.
pub const GUEST_SOCKET: &str = "/run/boxquell-ssh/agent.sock";

/// The socket of this computer's SSH agent, when one is running: the one
/// this process was started with, or one in the usual places of the desktop's
/// session, so an agent started later is found without a restart.
pub fn host_agent() -> Option<PathBuf> {
    let named = std::env::var_os("SSH_AUTH_SOCK").map(PathBuf::from);
    let session = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    let usual = ["ssh-agent.socket", "gcr/ssh", "keyring/ssh", "openssh_agent"].into_iter().filter_map(|name| Some(session.as_ref()?.join(name)));
    named.into_iter().chain(usual).find(|socket| socket.exists())
}

/// Whether a private key file asks for a passphrase. `None` when it is no readable key.
pub fn needs_passphrase(file: &Path) -> Option<bool> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut lines = text.lines();
    if !lines.next()?.contains("PRIVATE KEY") {
        return None;
    }
    // The current format names its cipher at the start: "none" for an open key.
    // The older PEM format says so in a header line.
    const OPEN_KEY: &str = "b3BlbnNzaC1rZXktdjEAAAAABG5vbmU";
    Some(text.contains("ENCRYPTED") || (text.contains("OPENSSH PRIVATE KEY") && !lines.next().unwrap_or_default().starts_with(OPEN_KEY)))
}

/// The private keys in `~/.ssh`, to choose from.
pub fn host_keys() -> Vec<Value> {
    let Some(dir) = std::env::home_dir().map(|home| home.join(".ssh")) else { return Vec::new() };
    let mut keys: Vec<Value> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_none())
        .filter_map(|path| {
            let protected = needs_passphrase(&path)?;
            // The public half ends in a comment, usually `user@computer`.
            let public = std::fs::read_to_string(path.with_extension("pub")).unwrap_or_default();
            let mut parts = public.split_whitespace();
            let kind = parts.next().unwrap_or_default().trim_start_matches("ssh-").to_string();
            let comment = parts.nth(1).unwrap_or_default().to_string();
            Some(json!({ "path": path, "kind": kind, "comment": comment, "passphrase": protected }))
        })
        .collect();
    keys.sort_by_key(|key| key["path"].as_str().unwrap_or_default().to_string());
    keys
}

/// What keeps the chosen keys from working, in words for the user.
pub fn problems(keys: &SshKeys) -> Vec<String> {
    let mut problems = Vec::new();
    if keys.is_off() {
        return problems;
    }
    if cfg!(not(unix)) {
        problems.push("SSH keys for sandboxes are not available on Windows yet.".to_string());
        return problems;
    }
    if keys.agent && host_agent().is_none() {
        let fallback = if keys.files.is_empty() { "" } else { " The chosen key files are used instead." };
        problems.push(format!("This computer's SSH agent is shared with the sandbox, but none is running.{fallback}"));
    }
    if !keys.agent || host_agent().is_none() {
        for file in &keys.files {
            match needs_passphrase(file) {
                None => problems.push(format!("The SSH key {} cannot be read.", file.display())),
                Some(true) => problems.push(format!(
                    "The SSH key {} has a passphrase: load it into your SSH agent (ssh-add) and share the agent instead.",
                    file.display()
                )),
                Some(false) => {}
            }
        }
    }
    problems
}

/// Keep the guest's socket connected to an agent on this computer for as long
/// as this future lives. Dropping it ends the connection and a private agent.
#[cfg(unix)]
pub async fn serve(key: PathBuf, ssh_port: u16, sandbox_dir: PathBuf, keys: SshKeys) {
    use std::process::Stdio;
    use tokio::process::Command;

    if keys.is_off() {
        return;
    }
    // The user's agent when it is shared and there; otherwise one of our own
    // that holds exactly the chosen files.
    let mut _private_agent = None;
    let socket = match host_agent().filter(|_| keys.agent) {
        Some(socket) => socket,
        None => {
            let socket = private_socket(&sandbox_dir);
            let _ = std::fs::remove_file(&socket);
            let agent = Command::new("ssh-agent")
                .arg("-D")
                .arg("-a")
                .arg(&socket)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn();
            let Ok(agent) = agent else {
                eprintln!("boxquell: ssh-agent could not be started; SSH keys are not available in the sandbox");
                return;
            };
            _private_agent = Some((agent, RemoveOnDrop(socket.clone())));
            for _ in 0..50 {
                if socket.exists() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            for file in &keys.files {
                // Never ask for a passphrase: nobody is there to answer.
                let added = Command::new("ssh-add")
                    .arg(file)
                    .env("SSH_AUTH_SOCK", &socket)
                    .env("SSH_ASKPASS_REQUIRE", "never")
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .await;
                if !added.is_ok_and(|status| status.success()) {
                    eprintln!("boxquell: the SSH key {} could not be loaded", file.display());
                }
            }
            socket
        }
    };

    let mut args = crate::ssh::base_args(&key, ssh_port);
    let destination = args.pop();
    for option in ["ExitOnForwardFailure=yes", "ServerAliveInterval=10", "ServerAliveCountMax=3"] {
        args.push("-o".into());
        args.push(option.into());
    }
    args.push("-N".into());
    args.push("-R".into());
    args.push(format!("{GUEST_SOCKET}:{}", socket.display()).into());
    args.extend(destination);
    loop {
        // Ends while the guest still boots, when it pauses and when it stops; try again.
        let _ = Command::new("ssh").args(&args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).status().await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

#[cfg(not(unix))]
pub async fn serve(_key: PathBuf, _ssh_port: u16, _sandbox_dir: PathBuf, _keys: SshKeys) {}

#[cfg(unix)]
struct RemoveOnDrop(PathBuf);

#[cfg(unix)]
impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// A short path for the private agent's socket; socket paths are limited to about 100 characters.
#[cfg(unix)]
fn private_socket(sandbox_dir: &Path) -> PathBuf {
    let path = sandbox_dir.join("ssh-agent.sock");
    if path.as_os_str().len() < 100 {
        return path;
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let name = sandbox_dir.file_name().unwrap_or_default().to_string_lossy();
    base.join(format!("boxquell-{}-{name}-agent.sock", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_open_keys_from_protected_ones() {
        let dir = std::env::temp_dir().join(format!("boxquell-keyagent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, text: &str| {
            let file = dir.join(name);
            std::fs::write(&file, text).unwrap();
            file
        };
        let open = write(
            "open",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAABG5vbmUAAAAEbm9uZQAAAAAAAAAB\n-----END OPENSSH PRIVATE KEY-----\n",
        );
        let protected = write(
            "protected",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaC1rZXktdjEAAAAACmFlczI1Ni1jdHIAAAAGYmNyeXB0\n-----END OPENSSH PRIVATE KEY-----\n",
        );
        let pem = write(
            "pem",
            "-----BEGIN RSA PRIVATE KEY-----\nProc-Type: 4,ENCRYPTED\nDEK-Info: AES-128-CBC,00\n\nAAAA\n-----END RSA PRIVATE KEY-----\n",
        );
        let other = write("config", "Host example\n  User me\n");
        assert_eq!(needs_passphrase(&open), Some(false));
        assert_eq!(needs_passphrase(&protected), Some(true));
        assert_eq!(needs_passphrase(&pem), Some(true));
        assert_eq!(needs_passphrase(&other), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
