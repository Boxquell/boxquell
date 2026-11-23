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
