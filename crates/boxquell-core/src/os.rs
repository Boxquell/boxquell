//! Operating systems for sandboxes. A definition (a TOML file in the `os`
//! directory of a recipe repository) names a container image and a setup
//! script; building it yields the disk image new sandboxes are copied from.

use crate::Home;
use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The system used when nothing else is chosen.
pub const DEFAULT: &str = "debian";
/// Recipes without a list of families are written for this one.
pub const DEFAULT_FAMILY: &str = "debian";

/// Scripts every guest needs: path in the guest and contents.
pub const GUEST_SCRIPTS: [(&str, &str); 3] = [
    ("/usr/local/sbin/boxquell-init", include_str!("../../../image/boxquell-init")),
    ("/usr/local/sbin/boxquell-shutdown", include_str!("../../../image/boxquell-shutdown")),
    ("/usr/local/sbin/boxquell-agent-config", include_str!("../../../image/boxquell-agent-config")),
];

/// What boxquell adds to every system after the definition's own setup.
const COMMON_SETUP: &str = r#"set -e
if ! id dev >/dev/null 2>&1; then
  useradd -m -s /bin/bash dev 2>/dev/null || adduser -D -s /bin/bash dev
fi
# Logging in with a key needs an account that is not locked.
usermod -p '*' dev 2>/dev/null || true
mkdir -p /etc/sudoers.d /workspace /etc/ssh/sshd_config.d /run/sshd
echo 'dev ALL=(ALL) NOPASSWD:ALL' > /etc/sudoers.d/dev
chmod 440 /etc/sudoers.d/dev
chown dev /workspace
cp /etc/ssl/certs/ca-certificates.crt /etc/ssl/boxquell-base-ca.crt
printf '%s\n' 'PasswordAuthentication no' 'PermitRootLogin no' > /etc/ssh/sshd_config.d/boxquell.conf
grep -q 'sshd_config.d' /etc/ssh/sshd_config || echo 'Include /etc/ssh/sshd_config.d/*.conf' >> /etc/ssh/sshd_config
chmod 755 /usr/local/sbin/boxquell-*
"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OsDef {
    /// Taken from the file name.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub description: String,
    /// Which recipes fit: they name the families they support.
    pub family: String,
    /// Hosts of the system's package manager. Recipes ask for them as `@packages`.
    #[serde(default)]
    pub package_hosts: Vec<String>,
    /// The container image the system is built from.
    pub image: String,
    /// Runs as root while the image is built.
    pub setup: String,
    /// The repository the definition comes from.
    #[serde(default)]
    pub source: String,
    /// Whether its disk image exists.
    #[serde(default)]
    pub built: bool,
}

/// What is recorded next to a built image.
#[derive(Debug, Serialize, Deserialize)]
struct Built {
    name: String,
    family: String,
    #[serde(default)]
    package_hosts: Vec<String>,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The disk image of a system. Images from before systems could be chosen are `base.img`.
pub fn image(home: &Home, id: &str) -> PathBuf {
    let path = home.root().join("images").join(format!("{id}.img"));
    if id == DEFAULT && !path.exists() {
        return home.base_image();
    }
    path
}

/// The family of a built system; recipes are checked against it.
pub fn family(home: &Home, id: &str) -> String {
    let record = std::fs::read(home.root().join("images").join(format!("{id}.json"))).ok();
    record.and_then(|bytes| serde_json::from_slice::<Built>(&bytes).ok()).map_or_else(|| DEFAULT_FAMILY.to_string(), |built| built.family)
}

/// The package manager hosts of a built system, which stand in for `@packages` in recipes.
pub fn package_hosts(home: &Home, id: &str) -> Vec<String> {
    let record = std::fs::read(home.root().join("images").join(format!("{id}.json"))).ok();
    let hosts = record.and_then(|bytes| serde_json::from_slice::<Built>(&bytes).ok()).map(|built| built.package_hosts).unwrap_or_default();
    // Images from before this was recorded are Debian.
    if hosts.is_empty() && family(home, id) == DEFAULT_FAMILY {
        return vec!["deb.debian.org".into(), "security.debian.org".into()];
    }
    hosts
}

/// Every definition in the added repositories. On equal ids the official repository wins.
pub fn definitions(home: &Home) -> Vec<OsDef> {
    let mut repos: Vec<PathBuf> =
        std::fs::read_dir(home.repos()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join("os").is_dir()).collect();
    repos.sort_by_key(|path| (!path.file_name().and_then(|n| n.to_str()).is_some_and(crate::repo::is_official), path.clone()));
    let mut all: Vec<OsDef> = Vec::new();
    for repo in repos {
        let source = repo.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let mut files: Vec<PathBuf> = std::fs::read_dir(repo.join("os"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        for file in files {
            let Some(id) = file.file_stem().and_then(|s| s.to_str()).filter(|id| valid_id(id)).map(str::to_string) else { continue };
            if all.iter().any(|def| def.id == id) {
                continue;
            }
            if let Ok(mut def) = std::fs::read_to_string(&file).map_err(anyhow::Error::from).and_then(|text| Ok(toml::from_str::<OsDef>(&text)?)) {
                def.built = image(home, &id).exists();
                def.id = id;
                def.source = source.clone();
                all.push(def);
            }
        }
    }
    // An image built before systems could be chosen has no definition at hand; show it anyway.
    if !all.iter().any(|def| def.id == DEFAULT) && image(home, DEFAULT).exists() {
        all.insert(
            0,
            OsDef {
                id: DEFAULT.into(),
                name: "Debian".into(),
                description: "The image already on this computer.".into(),
                family: DEFAULT_FAMILY.into(),
                package_hosts: Vec::new(),
                image: String::new(),
                setup: String::new(),
                source: String::new(),
                built: true,
            },
        );
    }
    all
}

/// A tool of e2fsprogs (`mke2fs`, `e2fsck`, `debugfs`). On macOS Homebrew
/// installs them outside the PATH, so those places are looked at as well.
pub fn e2fs_tool(name: &str) -> PathBuf {
    let places = ["/opt/homebrew/opt/e2fsprogs/sbin", "/usr/local/opt/e2fsprogs/sbin", "/opt/homebrew/sbin", "/usr/local/sbin"];
    let on_path = std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()));
    if on_path {
        return PathBuf::from(name);
    }
    places.iter().map(|dir| Path::new(dir).join(name)).find(|path| path.is_file()).unwrap_or_else(|| PathBuf::from(name))
}

fn run(program: &str, args: &[&std::ffi::OsStr], quiet: bool) -> Result<String> {
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null());
    if !quiet {
        // Building takes minutes; let the tool's own progress through.
        command.stderr(Stdio::inherit());
    }
    let output = command.output().with_context(|| format!("cannot run {program}; is it installed?"))?;
    if !output.status.success() {
        bail!(
            "{program} {} failed{}",
            args.first().map(|a| a.to_string_lossy()).unwrap_or_default(),
            if quiet { format!(": {}", String::from_utf8_lossy(&output.stderr).trim()) } else { String::new() }
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Build the disk image of a system. Needs Docker (to assemble the files) and
/// `mke2fs` from e2fsprogs 1.47.1 or newer (to pack them without root rights).
pub fn build(home: &Home, id: &str, size_gib: u32) -> Result<PathBuf> {
    let def = definitions(home)
        .into_iter()
        .find(|def| def.id == id && !def.image.is_empty())
        .with_context(|| format!("no system named {id:?}; see `boxquell os ls`"))?;
    ensure!(def.image.bytes().all(|b| b.is_ascii_alphanumeric() || b"./:_-@".contains(&b)), "invalid container image name {:?}", def.image);
    let images = home.root().join("images");
    let work = images.join(format!(".build-{id}"));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let result = (|| -> Result<PathBuf> {
        let names: Vec<&str> = GUEST_SCRIPTS.iter().filter_map(|(path, _)| path.rsplit('/').next()).collect();
        for ((_, contents), name) in GUEST_SCRIPTS.iter().zip(&names) {
            std::fs::write(work.join(name), contents)?;
        }
        std::fs::write(work.join("setup.sh"), format!("set -e\n{}\n", def.setup))?;
        std::fs::write(work.join("common.sh"), COMMON_SETUP)?;
        let dockerfile = format!(
            "FROM {}\nCOPY setup.sh common.sh /tmp/boxquell/\nRUN sh /tmp/boxquell/setup.sh\nCOPY {} /usr/local/sbin/\nRUN sh /tmp/boxquell/common.sh && rm -rf /tmp/boxquell\n",
            def.image,
            names.join(" ")
        );
        std::fs::write(work.join("Dockerfile"), dockerfile)?;

        let tag = format!("boxquell-os-{id}");
        run("docker", &["build".as_ref(), "-t".as_ref(), tag.as_ref(), work.as_os_str()], false)?;
        let container = run("docker", &["create".as_ref(), tag.as_ref()], true)?;
        let tarball = work.join("rootfs.tar");
        let exported = run("docker", &["export".as_ref(), container.as_ref(), "-o".as_ref(), tarball.as_os_str()], true);
        let _ = run("docker", &["rm".as_ref(), "-f".as_ref(), container.as_ref()], true);
        exported?;

        let partial = work.join("disk.img");
        let size = format!("{size_gib}G");
        pack_image(&work, &size)?;

        // A kernel in the image makes the system usable on the qemu backend.
        extract_boot_files(&tarball, &work.join("boot"));
        let boot = |prefix: &str| {
            std::fs::read_dir(work.join("boot"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(prefix)))
        };
        let target = images.join(format!("{id}.img"));
        for (prefix, extension) in [("vmlinuz-", "vmlinuz"), ("initrd.img-", "initrd")] {
            match boot(prefix) {
                Some(file) => drop(std::fs::copy(file, target.with_extension(extension))?),
                None => drop(std::fs::remove_file(target.with_extension(extension))),
            }
        }
        std::fs::rename(&partial, &target)?;
        std::fs::write(
            target.with_extension("json"),
            serde_json::to_vec_pretty(&Built { name: def.name.clone(), family: def.family.clone(), package_hosts: def.package_hosts.clone() })?,
        )?;
        Ok(target)
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Turn `rootfs.tar` in `work` into the ext4 image `disk.img`, keeping file
/// owners without root rights. That needs mke2fs 1.47.1 or newer. Where the
/// computer has none (Windows, a Mac without e2fsprogs, older Linux), the same
/// step runs in a container: Docker is there anyway, it built the files.
fn pack_image(work: &Path, size: &str) -> Result<()> {
    let (tarball, image) = (work.join("rootfs.tar"), work.join("disk.img"));
    let local = run(
        &e2fs_tool("mke2fs").to_string_lossy(),
        &[
            "-q".as_ref(),
            "-t".as_ref(),
            "ext4".as_ref(),
            "-L".as_ref(),
            "boxquell-root".as_ref(),
            "-d".as_ref(),
            tarball.as_os_str(),
            image.as_os_str(),
            size.as_ref(),
        ],
        true,
    );
    if local.is_ok() && image.exists() {
        return Ok(());
    }
    let _ = std::fs::remove_file(&image);
    let mount = format!("{}:/work", work.display());
    let script = format!(
        "set -e; apt-get update -qq >/dev/null; DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends e2fsprogs >/dev/null; \
         mke2fs -q -t ext4 -L boxquell-root -d /work/rootfs.tar /work/disk.img {size}; chown --reference=/work /work/disk.img"
    );
    run(
        "docker",
        &[
            "run".as_ref(),
            "--rm".as_ref(),
            "-v".as_ref(),
            mount.as_ref(),
            "debian:stable-slim".as_ref(),
            "sh".as_ref(),
            "-c".as_ref(),
            script.as_ref(),
        ],
        false,
    )
    .context("packing the disk image failed, with the computer's own mke2fs and in a container")?;
    ensure!(image.exists(), "packing the disk image produced no file");
    Ok(())
}

/// Copy the kernel and initrd out of a root filesystem archive into `target`.
fn extract_boot_files(tarball: &Path, target: &Path) {
    let Ok(file) = std::fs::File::open(tarball) else { return };
    let mut archive = tar::Archive::new(file);
    let Ok(entries) = archive.entries() else { return };
    let _ = std::fs::create_dir_all(target);
    for mut entry in entries.flatten() {
        let Ok(path) = entry.path().map(|p| p.into_owned()) else { continue };
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let in_boot = path.parent().is_some_and(|dir| dir.ends_with("boot"));
        if in_boot && entry.header().entry_type().is_file() && (name.starts_with("vmlinuz-") || name.starts_with("initrd.img-")) {
            if let Ok(mut out) = std::fs::File::create(target.join(&name)) {
                let _ = std::io::copy(&mut entry, &mut out);
            }
        }
    }
}

/// Whether a system brings the kernel the qemu backend needs.
pub fn has_kernel(image: &Path) -> bool {
    image.with_extension("vmlinuz").exists() && image.with_extension("initrd").exists()
}
