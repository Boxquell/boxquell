use anyhow::{bail, Context, Result};
use boxquell_net::{Mode, Rule};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Krun,
    Qemu,
}

impl Backend {
    /// libkrun where it exists (Linux, Apple Silicon), QEMU everywhere else.
    pub fn default_for_host() -> Self {
        if cfg!(target_os = "linux") || cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Backend::Krun
        } else {
            Backend::Qemu
        }
    }
}

/// `host` on 127.0.0.1 of this computer leads to `guest` inside the sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PortForward {
    pub host: u16,
    pub guest: u16,
    /// Address of this computer to listen on; 127.0.0.1 when unset, so only this computer can connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<std::net::IpAddr>,
}

/// A directory of this computer mounted in the guest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub host: PathBuf,
    /// Absolute path in the guest.
    pub guest: String,
    /// Mounted read-only. This guards against accidents; it is not a barrier
    /// against a program with root rights in the guest.
    #[serde(default)]
    pub read_only: bool,
}

/// Check a path in the guest that ends up in shell scripts and mount commands.
pub fn validate_guest_path(path: &str) -> Result<()> {
    let safe = path.starts_with('/')
        && path.len() <= 200
        && path
            .split('/')
            .skip(1)
            .all(|part| !part.is_empty() && part != "." && part != ".." && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b)));
    if !safe {
        bail!("invalid path in the sandbox {path:?}: use an absolute path of letters, digits, '.', '_' and '-'");
    }
    if ["/proc", "/sys", "/dev", "/run", "/etc", "/usr", "/bin", "/sbin", "/lib", "/boot"]
        .iter()
        .any(|root| path == *root || path.starts_with(&format!("{root}/")))
    {
        bail!("{path} is a system directory of the sandbox; choose another place, e.g. /mnt/data");
    }
    Ok(())
}

fn default_os() -> String {
    crate::os::DEFAULT.to_string()
}

fn default_disk_gib() -> u32 {
    20
}

/// Which SSH keys of this computer a sandbox may sign with, e.g. for git over
/// SSH. The keys never enter the sandbox: it gets a socket that signs for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshKeys {
    /// Every key loaded in this computer's SSH agent.
    pub agent: bool,
    /// Private key files of this computer, used when the agent is not shared.
    pub files: Vec<PathBuf>,
}

impl SshKeys {
    pub fn is_off(&self) -> bool {
        !self.agent && self.files.is_empty()
    }

    /// What a sandbox gets: the keys for all sandboxes plus its own.
    pub fn with(&self, other: &SshKeys) -> SshKeys {
        let mut files = self.files.clone();
        files.extend(other.files.iter().filter(|file| !self.files.contains(file)).cloned());
        SshKeys { agent: self.agent || other.agent, files }
    }
}

/// Everything that defines a sandbox; stored as `sandbox.toml` in its directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSpec {
    pub name: String,
    /// Host directory shared into the guest at `/workspace`.
    pub workspace: Option<PathBuf>,
    pub cpus: u8,
    pub ram_mib: u32,
    pub backend: Backend,
    /// The operating system the sandbox was created from; see `boxquell os ls`.
    #[serde(default = "default_os")]
    pub os: String,
    /// Size the sandbox disk is grown to; the guest enlarges its filesystem at boot.
    #[serde(default = "default_disk_gib")]
    pub disk_gib: u32,
    /// Whether destinations without a rule are blocked (whitelist) or let through (blacklist).
    #[serde(default)]
    pub network: Mode,
    /// Network rules, first match wins, everything else is denied.
    #[serde(default)]
    pub rules: Vec<Rule>,
    /// Names of host secrets whose placeholders are exported into the guest.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// More directories of this computer for the guest, besides the workspace.
    #[serde(default)]
    pub shares: Vec<Share>,
    /// Directory in the guest where shells and agents start; `/workspace` when unset.
    #[serde(default)]
    pub workdir: Option<String>,
    /// Started inside the guest whenever the sandbox boots: names of installed
    /// agents (their recipe's start command is used) or shell commands.
    #[serde(default)]
    pub autostart: Vec<String>,
    /// Ports of this computer (127.0.0.1) that lead to a port in the guest.
    #[serde(default)]
    pub ports: Vec<PortForward>,
    /// Recipes installed into this sandbox.
    #[serde(default)]
    pub recipes: Vec<String>,
    /// Rules that end by themselves: the rule's text with the time it ends, in seconds since 1970.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub rule_expiry: BTreeMap<String, u64>,
    /// How much the sandbox may send to destinations outside per run, in MiB.
    /// Beyond it the network is cut off until someone lets it continue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upload_limit_mib: Option<u64>,
    /// The same for what it may receive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub download_limit_mib: Option<u64>,
    /// Stop the sandbox after this many minutes without work in it; 0 never
    /// stops it. Left out, the setting for all sandboxes applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_stop_minutes: Option<u64>,
    /// Only when a sandbox is created: the template its disk and software come from.
    #[serde(default, skip_serializing)]
    pub template: Option<String>,
    /// The fingerprint each installed recipe had when it was installed, to
    /// notice later that the recipe has changed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub recipe_revisions: BTreeMap<String, String>,
    /// Destinations that installed software and the system open by themselves
    /// and that the user switched off: a source such as `tools:gh` or `os` for
    /// all of its destinations, or `<source> <destination>` for one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_off: Vec<String>,
    /// Extra environment variables for sessions in the guest.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// SSH keys of this computer the sandbox may sign with, besides the global ones.
    #[serde(default, skip_serializing_if = "SshKeys::is_off")]
    pub ssh: SshKeys,
}

pub fn validate_name(name: &str) -> Result<()> {
    let ok =
        (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !name.starts_with('-');
    if !ok {
        bail!("invalid sandbox name {name:?}: use 1-32 characters of a-z, 0-9 and '-'");
    }
    Ok(())
}

impl SandboxSpec {
    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join("sandbox.toml");
        let text = std::fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        toml::from_str(&text).with_context(|| format!("invalid {}", path.display()))
    }

    pub fn save(&self, dir: &Path) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("sandbox.toml"), toml::to_string_pretty(self)?)?;
        Ok(())
    }
}
