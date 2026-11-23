//! VM backends for boxquell: libkrun on Linux and Apple Silicon, QEMU elsewhere.
//!
//! Both boot the same raw disk image, attach the config drive as a second
//! disk and connect the guest NIC to the gateway's stream socket.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(unix)]
pub mod krun;
pub mod qemu;

/// The guest's first process, part of the base image.
pub const GUEST_INIT: &str = "/usr/local/sbin/boxquell-init";
/// virtiofs tag of the shared project directory.
pub const WORKSPACE_TAG: &str = "workspace";
pub const GUEST_MAC: [u8; 6] = [0x52, 0x54, 0x00, 0xcb, 0x00, 0x01];

/// Where the gateway listens for the VM's network link.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetLink {
    Unix(PathBuf),
    /// Loopback TCP port, for hosts without unix sockets.
    Tcp(u16),
}

/// Everything a backend needs to boot one sandbox.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VmSpec {
    pub cpus: u8,
    pub ram_mib: u32,
    /// Raw ext4 image, mounted read-write as the guest root.
    pub disk: PathBuf,
    /// Tar archive with per-boot configuration, attached read-only.
    pub config_drive: PathBuf,
    pub net: NetLink,
    /// Host directory shared into the guest at /workspace.
    pub workspace: Option<PathBuf>,
}
