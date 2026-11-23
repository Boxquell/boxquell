//! State shared by the boxquell daemon and CLI: where things live on disk, what a
//! sandbox is made of, the event log and the secrets kept on the host.

pub mod agentcfg;
pub mod claude;
pub mod drive;
pub mod home;
pub mod recipe;
pub mod repo;
pub mod secrets;
pub mod spec;
pub mod store;

pub use home::Home;
pub use secrets::{SecretDef, SecretStore};
pub use spec::{Backend, PortForward, SandboxSpec, Share};
pub use store::{EventFilter, EventStore};

/// Create or replace a file that only the current user may read.
pub fn write_private(path: &std::path::Path, contents: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let _ = std::fs::remove_file(path);
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
        file.write_all(contents)
    }
    #[cfg(not(unix))]
    {
        // The boxquell home lives in the user profile, which other users cannot read.
        std::fs::write(path, contents)
    }
}
