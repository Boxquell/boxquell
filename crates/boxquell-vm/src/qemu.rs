//! QEMU backend. Used where libkrun is not available (Windows, Intel Macs)
//! and as a second backend on Linux.

use crate::{NetLink, VmSpec, GUEST_INIT, GUEST_MAC};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Build the QEMU invocation. `kernel` and `initrd` come with the base image.
pub fn command(spec: &VmSpec, kernel: &Path, initrd: &Path) -> Command {
    // microvm boots fastest but is only exercised with KVM; other accelerators get q35.
    let (machine, bus) = if cfg!(target_os = "linux") {
        ("microvm,accel=kvm", "device")
    } else if cfg!(target_os = "macos") {
        ("q35,accel=hvf", "pci")
    } else {
        ("q35,accel=whpx", "pci")
    };
    let cpu = if cfg!(target_os = "windows") { "max" } else { "host" };
    let mac = GUEST_MAC.map(|b| format!("{b:02x}")).join(":");
    let netdev = match &spec.net {
        NetLink::Unix(path) => format!("stream,id=net0,server=off,addr.type=unix,addr.path={}", path.display()),
        NetLink::Tcp(port) => format!("stream,id=net0,server=off,addr.type=inet,addr.host=127.0.0.1,addr.port={port}"),
    };
    let drive = |id: &str, path: &Path, extra: &str| {
        let mut arg = OsString::from(format!("id={id},format=raw,if=none{extra},file="));
        arg.push(path);
        arg
    };

    let mut cmd = Command::new("qemu-system-x86_64");
    cmd.args(["-M", machine, "-cpu", cpu, "-nodefaults", "-no-user-config", "-nographic", "-no-reboot"])
        .args(["-smp", &spec.cpus.to_string(), "-m", &spec.ram_mib.to_string()])
        .args(["-serial", "stdio"])
        .arg("-kernel")
        .arg(kernel)
        .arg("-initrd")
        .arg(initrd)
        .args(["-append", &format!("console=ttyS0 quiet root=/dev/vda rw init={GUEST_INIT}")])
        .arg("-drive")
        .arg(drive("root", &spec.disk, ""))
        .args(["-device", &format!("virtio-blk-{bus},drive=root")])
        .arg("-drive")
        .arg(drive("config", &spec.config_drive, ",readonly=on"))
        .args(["-device", &format!("virtio-blk-{bus},drive=config")])
        .args(["-netdev", &netdev])
        .args(["-device", &format!("virtio-net-{bus},netdev=net0,mac={mac}")]);
    // Folders of this computer reach the guest through QEMU's own file server (9p), which needs
    // no helper program. The guest mounts each by its tag.
    if SHARES_FOLDERS {
        let folders = spec.workspace.iter().map(|dir| (crate::WORKSPACE_TAG, dir)).chain(spec.shares.iter().map(|(tag, dir)| (tag.as_str(), dir)));
        for (i, (tag, dir)) in folders.enumerate() {
            // A comma ends an option; a doubled one stands for itself.
            let mut fsdev = OsString::from(format!("local,id=fs{i},security_model=none,path="));
            fsdev.push(dir.to_string_lossy().replace(',', ",,"));
            cmd.arg("-fsdev").arg(fsdev).args(["-device", &format!("virtio-9p-{bus},fsdev=fs{i},mount_tag={tag}")]);
        }
    }
    cmd
}

/// Whether QEMU can hand folders to a guest on this system. Its file server does not exist in the Windows builds.
pub const SHARES_FOLDERS: bool = cfg!(not(target_os = "windows"));
