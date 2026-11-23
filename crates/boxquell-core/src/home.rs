use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// A path from the environment variable `var`, with a leading `~` expanded.
/// Shells such as fish do not expand `~` in `export VAR=~/dir`.
pub fn env_path(var: &str) -> Option<PathBuf> {
    let raw = PathBuf::from(std::env::var_os(var).filter(|v| !v.is_empty())?);
    match (raw.strip_prefix("~"), std::env::home_dir()) {
        (Ok(rest), Some(home)) => Some(home.join(rest)),
        _ => Some(raw),
    }
}

/// The directory holding everything boxquell stores: images, sandboxes, the CA,
/// secrets and the event log. `BOXQUELL_HOME` overrides the per-user default.
#[derive(Debug, Clone)]
pub struct Home(PathBuf);

impl Home {
    pub fn locate() -> Result<Self> {
        let root = match env_path("BOXQUELL_HOME") {
            Some(dir) => dir,
            None => directories::ProjectDirs::from("", "", "boxquell")
                .context("cannot determine the user data directory; set BOXQUELL_HOME")?
                .data_dir()
                .to_path_buf(),
        };
        std::fs::create_dir_all(root.join("sandboxes")).with_context(|| format!("cannot create {}", root.display()))?;
        Ok(Self(std::path::absolute(root)?))
    }

    pub fn root(&self) -> &Path {
        &self.0
    }
    pub fn sandboxes(&self) -> PathBuf {
        self.0.join("sandboxes")
    }
    pub fn sandbox(&self, name: &str) -> PathBuf {
        self.sandboxes().join(name)
    }
    /// Base image; the QEMU kernel and initrd sit next to it as `base.vmlinuz` / `base.initrd`.
    pub fn base_image(&self) -> PathBuf {
        self.0.join("images").join("base.img")
    }
    pub fn recipes(&self) -> PathBuf {
        self.0.join("recipes")
    }
    /// Checkouts of recipe repositories, one directory each.
    pub fn repos(&self) -> PathBuf {
        self.0.join("repos")
    }
    pub fn daemon_file(&self) -> PathBuf {
        self.0.join("daemon.json")
    }
    pub fn events_db(&self) -> PathBuf {
        self.0.join("events.db")
    }
    pub fn secrets_file(&self) -> PathBuf {
        self.0.join("secrets.json")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilde_in_env_path_is_expanded() {
        let home = std::env::home_dir().unwrap();
        unsafe { std::env::set_var("BOXQUELL_TEST_PATH", "~/a/b") };
        assert_eq!(env_path("BOXQUELL_TEST_PATH"), Some(home.join("a/b")));
        unsafe { std::env::set_var("BOXQUELL_TEST_PATH", "/x/~/y") };
        assert_eq!(env_path("BOXQUELL_TEST_PATH"), Some(PathBuf::from("/x/~/y")));
    }
}
