//! `boxquell.toml`: a project's sandbox, described in a file that lives with
//! the project. `boxquell up` in the project folder creates the sandbox from
//! it; `boxquell init` writes it from a sandbox that exists.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::spec::SandboxSpec;

pub const FILE: &str = "boxquell.toml";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Project {
    /// The sandbox's name; the folder's name when left out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Operating system id, e.g. `debian`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// Recipes to install, e.g. `["claude", "node@24"]`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub software: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u8>,
    /// Memory in MiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<u32>,
    /// Disk in GiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disk: Option<u32>,
    /// `whitelist` (the default) or `blacklist`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Destinations to allow, to allow for reading only, and to deny.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub read: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
    /// Ports to forward from this computer: `3000` or `8080:80` (here:there).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ports: Vec<String>,
    /// Where shells and agents start in the sandbox.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workdir: Option<String>,
    /// Agents or commands that start with the sandbox.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub autostart: Vec<String>,
    /// Names of secrets stored on this computer to attach. Values never go into this file.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    /// Review mode: the sandbox works on a copy of the project folder, and its
    /// changes reach the project only when they are taken over.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review: Option<bool>,
    /// Stop the sandbox after this many minutes without work; 0 for never.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_stop: Option<u64>,
    /// Data limits per run, in MiB.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upload_limit: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_limit: Option<u64>,
    /// Environment variables for sessions in the sandbox.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// The project file of `dir` or of the nearest folder above it.
pub fn find(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().map(|folder| folder.join(FILE)).find(|file| file.is_file())
}

pub fn load(file: &Path) -> Result<Project> {
    let text = std::fs::read_to_string(file).with_context(|| format!("cannot read {}", file.display()))?;
    toml::from_str(&text).with_context(|| format!("{} is not a valid project file", file.display()))
}

impl Project {
    /// The sandbox's name: the one in the file, or one made from the folder's name.
    pub fn sandbox_name(&self, folder: &Path) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let raw = folder.file_name().map(|name| name.to_string_lossy().to_lowercase()).unwrap_or_default();
        let mut name: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        while name.contains("--") {
            name = name.replace("--", "-");
        }
        let name = name.trim_matches('-').to_string();
        if name.is_empty() {
            "project".to_string()
        } else {
            name
        }
    }

    /// The rules as the sandbox keeps them, e.g. `allow example.com`.
    pub fn rules(&self) -> Vec<String> {
        let with = |action: &str, hosts: &[String]| hosts.iter().map(|host| format!("{action} {host}")).collect::<Vec<_>>();
        [with("deny", &self.deny), with("read", &self.read), with("allow", &self.allow)].concat()
    }

    /// What a sandbox looks like as a project file. Software loses the name of
    /// its repository where that is an official one, so the file reads like
    /// one written by hand.
    pub fn of(spec: &SandboxSpec) -> Project {
        let short = |id: &String| match id.split_once(':') {
            Some((repo, rest)) if crate::repo::is_official(repo) => rest.to_string(),
            _ => id.clone(),
        };
        let hosts = |action: &str| -> Vec<String> {
            spec.rules.iter().map(|rule| rule.to_string()).filter_map(|rule| rule.strip_prefix(&format!("{action} ")).map(str::to_string)).collect()
        };
        Project {
            name: Some(spec.name.clone()),
            os: Some(spec.os.clone()),
            software: spec.recipes.iter().map(short).collect(),
            cpus: Some(spec.cpus),
            memory: Some(spec.ram_mib),
            disk: Some(spec.disk_gib),
            mode: Some(if spec.network == boxquell_net::Mode::Blacklist { "blacklist" } else { "whitelist" }.to_string()),
            allow: hosts("allow"),
            read: hosts("read"),
            deny: hosts("deny"),
            ports: spec
                .ports
                .iter()
                .map(|port| if port.host == port.guest { port.host.to_string() } else { format!("{}:{}", port.host, port.guest) })
                .collect(),
            workdir: spec.workdir.clone(),
            autostart: spec.autostart.clone(),
            secrets: spec.secrets.clone(),
            review: spec.review.then_some(true),
            idle_stop: spec.idle_stop_minutes,
            upload_limit: spec.upload_limit_mib,
            download_limit: spec.download_limit_mib,
            env: spec.env.clone(),
        }
    }

    pub fn to_toml(&self) -> Result<String> {
        let body = toml::to_string_pretty(self)?;
        Ok(format!(
            "# The sandbox of this project. `boxquell up` in this folder creates and starts it.\n# https://github.com/Boxquell/boxquell\n\n{body}"
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_what_it_writes_and_names_a_sandbox_after_its_folder() {
        let project: Project = toml::from_str(
            "software = ['claude', 'node@24']\nallow = ['github.com']\nread = ['docs.example.com']\nports = ['3000']\n[env]\nA = 'b'\n",
        )
        .unwrap();
        assert_eq!(project.rules(), ["read docs.example.com", "allow github.com"]);
        assert_eq!(project.sandbox_name(Path::new("/home/me/My Web_Shop")), "my-web-shop");
        let again: Project = toml::from_str(&project.to_toml().unwrap()).unwrap();
        assert_eq!(again, project);
        assert!(toml::from_str::<Project>("sofware = []").is_err());
    }
}
