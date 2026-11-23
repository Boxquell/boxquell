//! Credentials that never enter a sandbox. The guest gets the placeholder in
//! its environment; the gateway swaps it for the value on allowed hosts.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Serialize, Deserialize)]
pub struct SecretDef {
    pub name: String,
    /// Hosts the secret may be sent to (`api.example.com`, `*.example.com`).
    pub hosts: Vec<String>,
    /// Environment variables that carry the placeholder inside the guest.
    pub env: Vec<String>,
    pub placeholder: String,
    pub value: String,
}

impl std::fmt::Debug for SecretDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretDef").field("name", &self.name).field("hosts", &self.hosts).field("env", &self.env).finish_non_exhaustive()
    }
}

impl SecretDef {
    /// Known services: hosts and variable names are filled in from the name.
    pub fn preset(name: &str) -> Option<Self> {
        let (hosts, env, placeholder): (&[&str], &[&str], &str) = match name {
            "anthropic" => (&["api.anthropic.com"], &["ANTHROPIC_API_KEY"], "sk-ant-api03-boxquell-placeholder"),
            "claude-oauth" => (&["api.anthropic.com"], &["CLAUDE_CODE_OAUTH_TOKEN"], "sk-ant-oat01-boxquell-placeholder"),
            "openai" => (&["api.openai.com"], &["OPENAI_API_KEY", "CODEX_API_KEY"], "sk-boxquell-placeholder"),
            "openrouter" => (&["openrouter.ai"], &["OPENROUTER_API_KEY"], "sk-or-boxquell-placeholder"),
            "github" => (&["api.github.com"], &["GH_TOKEN", "GITHUB_TOKEN"], "ghp_boxquellplaceholder"),
            _ => return None,
        };
        Some(Self {
            name: name.to_string(),
            hosts: hosts.iter().map(|h| h.to_string()).collect(),
            env: env.iter().map(|e| e.to_string()).collect(),
            placeholder: placeholder.to_string(),
            value: String::new(),
        })
    }

    pub const PRESETS: [&'static str; 5] = ["anthropic", "claude-oauth", "openai", "openrouter", "github"];
}

/// Secrets file in the boxquell home, readable by the owner only.
pub struct SecretStore {
    path: PathBuf,
}

impl SecretStore {
    pub fn new(path: &Path) -> Self {
        Self { path: path.to_path_buf() }
    }

    pub fn list(&self) -> Result<Vec<SecretDef>> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| format!("invalid {}", self.path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(err) => Err(err).with_context(|| format!("cannot read {}", self.path.display())),
        }
    }

    pub fn get(&self, name: &str) -> Result<Option<SecretDef>> {
        Ok(self.list()?.into_iter().find(|s| s.name == name))
    }

    pub fn set(&self, secret: SecretDef) -> Result<()> {
        if secret.value.is_empty() || secret.hosts.is_empty() || secret.placeholder.is_empty() {
            bail!("a secret needs a value, a placeholder and at least one host");
        }
        let mut all = self.list()?;
        all.retain(|s| s.name != secret.name);
        all.push(secret);
        all.sort_by(|a, b| a.name.cmp(&b.name));
        self.save(&all)
    }

    /// Returns whether a secret of that name existed.
    pub fn remove(&self, name: &str) -> Result<bool> {
        let mut all = self.list()?;
        let before = all.len();
        all.retain(|s| s.name != name);
        self.save(&all)?;
        Ok(all.len() != before)
    }

    fn save(&self, all: &[SecretDef]) -> Result<()> {
        crate::write_private(&self.path, &serde_json::to_vec_pretty(all)?)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_remove() {
        let path = std::env::temp_dir().join(format!("boxquell-secrets-test-{}.json", std::process::id()));
        let store = SecretStore::new(&path);
        assert!(store.list().unwrap().is_empty());
        let mut secret = SecretDef::preset("anthropic").unwrap();
        assert!(store.set(secret.clone()).is_err(), "empty value is rejected");
        secret.value = "sk-real".into();
        store.set(secret).unwrap();
        assert_eq!(store.get("anthropic").unwrap().unwrap().value, "sk-real");
        assert!(!format!("{:?}", store.get("anthropic").unwrap()).contains("sk-real"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(store.remove("anthropic").unwrap());
        assert!(!store.remove("anthropic").unwrap());
        std::fs::remove_file(&path).unwrap();
    }
}
