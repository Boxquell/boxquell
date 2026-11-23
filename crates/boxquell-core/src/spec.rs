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
}

fn default_disk_gib() -> u32 {
    20
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
    /// Ports of this computer (127.0.0.1) that lead to a port in the guest.
    #[serde(default)]
    pub ports: Vec<PortForward>,
    /// Recipes installed into this sandbox.
    #[serde(default)]
    pub recipes: Vec<String>,
    /// Extra environment variables for sessions in the guest.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
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
