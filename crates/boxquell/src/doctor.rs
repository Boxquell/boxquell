//! `boxquell doctor`: check what a sandbox needs on this host and name the missing step.

use boxquell_core::home::Home;
use std::path::Path;

fn on_path(program: &str) -> bool {
    let exts: &[&str] = if cfg!(windows) { &["", ".exe"] } else { &[""] };
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| exts.iter().any(|ext| dir.join(format!("{program}{ext}")).is_file())))
}

fn line(ok: bool, what: &str, detail: &str) -> bool {
    println!("{} {what:<22} {detail}", if ok { "ok  " } else { "FAIL" });
    ok
}

fn exists(path: &Path) -> String {
    path.display().to_string()
}

/// Print one line per check; returns whether at least one backend can start a sandbox.
pub fn run(home: &Home) -> bool {
    line(true, "data directory", &exists(home.root()));

    let accel = if cfg!(target_os = "linux") {
        let usable = std::fs::OpenOptions::new().read(true).write(true).open("/dev/kvm").is_ok();
        line(
            usable,
            "KVM",
            if usable { "/dev/kvm is usable" } else { "/dev/kvm is missing or not writable: enable virtualization and join the `kvm` group" },
        )
    } else {
        line(true, "hypervisor", "not checked on this system (HVF on macOS, WHPX on Windows)")
    };

    #[cfg(unix)]
    let krun = {
        let library = boxquell_vm::krun::find_library(&home.root().join("lib"));
        match boxquell_vm::krun::probe(&library) {
            Ok(()) => {
                // libkrun loads the kernel it boots from a library of its own, at the first start of a sandbox.
                let kernel = library.parent().filter(|dir| !dir.as_os_str().is_empty()).map(|dir| {
                    std::fs::read_dir(dir).into_iter().flatten().flatten().any(|entry| entry.file_name().to_string_lossy().starts_with("libkrunfw."))
                });
                match kernel {
                    Some(false) => {
                        line(false, "libkrun", &format!("{} is there, but libkrunfw is not next to it; install libkrunfw", library.display()))
                    }
                    _ => line(true, "libkrun", &exists(&library)),
                }
            }
            Err(err) => line(false, "libkrun", &format!("{err:#}; install libkrun and libkrunfw or set BOXQUELL_KRUN_LIB")),
        }
    };
    #[cfg(not(unix))]
    let krun = line(false, "libkrun", "not available on this system; sandboxes use the qemu backend");

    let qemu_bin = on_path("qemu-system-x86_64");
    line(qemu_bin, "QEMU", if qemu_bin { "qemu-system-x86_64 found" } else { "qemu-system-x86_64 not on PATH (only needed for `--backend qemu`)" });

    let ssh = line(on_path("ssh"), "ssh", if on_path("ssh") { "found" } else { "not on PATH: install an OpenSSH client" });

    let systems: Vec<_> = boxquell_core::os::definitions(home).into_iter().filter(|def| def.built).collect();
    let image = if systems.is_empty() {
        line(false, "system image", "none yet: run `boxquell os install debian`")
    } else {
        line(true, "system image", &systems.iter().map(|def| def.id.as_str()).collect::<Vec<_>>().join(", "))
    };
    let kernel = systems.iter().any(|def| boxquell_core::os::has_kernel(&boxquell_core::os::image(home, &def.id)));
    line(kernel, "QEMU kernel", if kernel { "found" } else { "no built system brings one (only needed for `--backend qemu`)" });

    let daemon = home.daemon_file().exists();
    line(true, "daemon", if daemon { "running" } else { "not running (starts on demand)" });

    let krun_ready = accel && krun && ssh && image;
    let qemu_ready = accel && qemu_bin && ssh && image && kernel;
    println!();
    match (krun_ready, qemu_ready) {
        (true, true) => println!("Both backends (krun, qemu) are ready."),
        (true, false) => println!("The krun backend is ready; qemu is not."),
        (false, true) => println!("The qemu backend is ready; krun is not. Create sandboxes with `--backend qemu`."),
        (false, false) => println!("No backend is ready yet; fix the FAIL lines above."),
    }
    krun_ready || qemu_ready
}
