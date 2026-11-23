//! libkrun backend, bound at runtime to the libkrun 1.x C API.
//!
//! The library is loaded with dlopen so the binary starts on machines without
//! libkrun and can report what is missing instead of failing to link.
//! `krun_start_enter` takes over the calling process, so every VM runs in a
//! child process started through [`command`].

use crate::{NetLink, VmSpec, GUEST_INIT, GUEST_MAC, WORKSPACE_TAG};
use anyhow::{bail, Context, Result};
use libloading::Library;
use std::ffi::{c_char, c_int, CString, OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::ptr;

const DISK_FORMAT_RAW: u32 = 0;
const LOG_LEVEL_WARN: u32 = 2;

#[cfg(target_os = "macos")]
const LIB_NAME: &str = "libkrun.1.dylib";
#[cfg(not(target_os = "macos"))]
const LIB_NAME: &str = "libkrun.so.1";

macro_rules! krun_api {
    ($( fn $name:ident($($arg:ident: $ty:ty),*); )*) => {
        struct Api {
            _lib: Library,
            $( $name: unsafe extern "C" fn($($ty),*) -> i32, )*
        }
        impl Api {
            unsafe fn load(path: &OsStr) -> Result<Self, libloading::Error> {
                let lib = Library::new(path)?;
                $( let $name = *lib.get::<unsafe extern "C" fn($($ty),*) -> i32>(
                    concat!(stringify!($name), "\0").as_bytes())?; )*
                Ok(Self { _lib: lib, $($name,)* })
            }
        }
    };
}

krun_api! {
    fn krun_set_log_level(level: u32);
    fn krun_create_ctx();
    fn krun_set_vm_config(ctx: u32, vcpus: u8, ram_mib: u32);
    fn krun_add_disk2(ctx: u32, id: *const c_char, path: *const c_char, format: u32, read_only: bool);
    fn krun_set_root_disk_remount(ctx: u32, device: *const c_char, fstype: *const c_char, options: *const c_char);
    fn krun_add_virtiofs(ctx: u32, tag: *const c_char, path: *const c_char);
    fn krun_add_net_unixstream(ctx: u32, path: *const c_char, fd: c_int, mac: *mut u8, features: u32, flags: u32);
    fn krun_disable_implicit_vsock(ctx: u32);
    fn krun_set_workdir(ctx: u32, path: *const c_char);
    fn krun_set_exec(ctx: u32, exec: *const c_char, argv: *const *const c_char, envp: *const *const c_char);
    fn krun_start_enter(ctx: u32);
}

/// Locate libkrun: `BOXQUELL_KRUN_LIB`, then `lib_dir`, then the system's library path.
pub fn find_library(lib_dir: &Path) -> PathBuf {
    if let Some(path) = std::env::var_os("BOXQUELL_KRUN_LIB").filter(|v| !v.is_empty()).map(PathBuf::from) {
        // Shells such as fish do not expand `~` in `export VAR=~/dir`.
        return match (path.strip_prefix("~"), std::env::home_dir()) {
            (Ok(rest), Some(home)) => home.join(rest),
            _ => path,
        };
    }
    let bundled = lib_dir.join(LIB_NAME);
    if bundled.exists() {
        bundled
    } else {
        PathBuf::from(LIB_NAME)
    }
}

/// Whether libkrun can be loaded from `library`.
pub fn probe(library: &Path) -> Result<()> {
    unsafe { Api::load(library.as_os_str()) }.map(drop).with_context(|| format!("cannot load libkrun ({})", library.display()))
}

/// The child process that becomes the VM: `<exe> vm-run <library> <spec json>`.
pub fn command(exe: &Path, library: &Path, spec_json: &str) -> Command {
    let mut cmd = Command::new(exe);
    cmd.arg("vm-run").arg(library).arg(spec_json);
    // libkrun loads libkrunfw by name; make a bundled copy next to it findable.
    if let Some(dir) = library.parent().filter(|d| !d.as_os_str().is_empty()) {
        let var = if cfg!(target_os = "macos") { "DYLD_LIBRARY_PATH" } else { "LD_LIBRARY_PATH" };
        let mut value = OsString::from(dir);
        if let Some(old) = std::env::var_os(var) {
            value.push(":");
            value.push(old);
        }
        cmd.env(var, value);
    }
    cmd
}

fn cstr(s: impl AsRef<[u8]>) -> Result<CString> {
    CString::new(s.as_ref()).context("string contains a NUL byte")
}

fn path_cstr(p: &Path) -> Result<CString> {
    cstr(p.as_os_str().as_encoded_bytes())
}

fn check(what: &str, rc: i32) -> Result<i32> {
    if rc < 0 {
        bail!("{what} failed: {}", std::io::Error::from_raw_os_error(-rc));
    }
    Ok(rc)
}

/// Boot the VM in the current process. Returns only on a configuration error:
/// on success libkrun takes over the process and exits it with the guest's exit code.
pub fn enter(library: &Path, spec: &VmSpec) -> Result<std::convert::Infallible> {
    let NetLink::Unix(socket) = &spec.net else {
        bail!("libkrun needs a unix socket for the network link");
    };
    let api = unsafe { Api::load(library.as_os_str()) }.with_context(|| format!("cannot load libkrun ({})", library.display()))?;
    unsafe {
        (api.krun_set_log_level)(LOG_LEVEL_WARN);
        let ctx = check("krun_create_ctx", (api.krun_create_ctx)())? as u32;
        check("krun_set_vm_config", (api.krun_set_vm_config)(ctx, spec.cpus, spec.ram_mib))?;

        // Guest device order follows the order here: root is /dev/vda, the config drive /dev/vdb.
        let disk = path_cstr(&spec.disk)?;
        check("krun_add_disk2(root)", (api.krun_add_disk2)(ctx, c"root".as_ptr(), disk.as_ptr(), DISK_FORMAT_RAW, false))?;
        let config = path_cstr(&spec.config_drive)?;
        check("krun_add_disk2(config)", (api.krun_add_disk2)(ctx, c"config".as_ptr(), config.as_ptr(), DISK_FORMAT_RAW, true))?;
        check("krun_set_root_disk_remount", (api.krun_set_root_disk_remount)(ctx, c"/dev/vda".as_ptr(), c"ext4".as_ptr(), ptr::null()))?;

        if let Some(dir) = &spec.workspace {
            let (tag, dir) = (cstr(WORKSPACE_TAG)?, path_cstr(dir)?);
            check("krun_add_virtiofs", (api.krun_add_virtiofs)(ctx, tag.as_ptr(), dir.as_ptr()))?;
        }

        for (tag, dir) in &spec.shares {
            let (tag, dir) = (cstr(tag)?, path_cstr(dir)?);
            check("krun_add_virtiofs", (api.krun_add_virtiofs)(ctx, tag.as_ptr(), dir.as_ptr()))?;
        }

        // Adding a net device turns TSI networking off; dropping the implicit vsock
        // removes the last channel that bypasses the gateway.
        let socket = path_cstr(socket)?;
        let mut mac = GUEST_MAC;
        check("krun_add_net_unixstream", (api.krun_add_net_unixstream)(ctx, socket.as_ptr(), -1, mac.as_mut_ptr(), 0, 0))?;
        check("krun_disable_implicit_vsock", (api.krun_disable_implicit_vsock)(ctx))?;

        // libkrun mangles arguments containing double quotes, so the guest gets
        // no arguments at all; its configuration travels on the config drive.
        check("krun_set_workdir", (api.krun_set_workdir)(ctx, c"/".as_ptr()))?;
        let exec = cstr(GUEST_INIT)?;
        let argv = [ptr::null::<c_char>()];
        let env = [c"PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".as_ptr(), ptr::null()];
        check("krun_set_exec", (api.krun_set_exec)(ctx, exec.as_ptr(), argv.as_ptr(), env.as_ptr()))?;

        check("krun_start_enter", (api.krun_start_enter)(ctx))?;
    }
    bail!("krun_start_enter returned without starting the VM")
}
