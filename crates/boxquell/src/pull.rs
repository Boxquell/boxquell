//! Ready-made system images. The repository with the system definitions
//! publishes the official systems as disk images, so a sandbox can be created
//! without Docker or any tool to build one. A download is checked against
//! the release's SHA256SUMS.

use anyhow::{bail, ensure, Context, Result};
use boxquell_core::Home;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

/// Where ready-made images are published: the latest release of the
/// repository that holds the system definitions. `BOXQUELL_IMAGE_URL` names another folder.
fn base_url() -> String {
    std::env::var("BOXQUELL_IMAGE_URL")
        .ok()
        .filter(|url| !url.is_empty())
        .unwrap_or_else(|| "https://github.com/Boxquell/boxquell-os/releases/latest/download".to_string())
}

fn agent() -> ureq::Agent {
    // No overall time limit: an image is a few hundred megabytes.
    ureq::Agent::config_builder().timeout_connect(Some(std::time::Duration::from_secs(20))).http_status_as_error(true).build().into()
}

/// The name of the image archive for this computer's processor.
pub fn asset_name(id: &str) -> String {
    format!("boxquell-os-{id}-{}.tar.gz", std::env::consts::ARCH)
}

/// Copy `from` to `to`, leaving runs of zeros as holes so an image takes only the space it uses.
fn copy_sparse(mut from: impl Read, to: &mut std::fs::File) -> Result<()> {
    let mut buf = vec![0u8; 1 << 20];
    let mut length = 0u64;
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if buf[..n].iter().all(|byte| *byte == 0) {
            to.seek(SeekFrom::Current(n as i64))?;
        } else {
            to.write_all(&buf[..n])?;
        }
        length += n as u64;
    }
    to.set_len(length)?;
    Ok(())
}

/// Download the image of an official system and put it where sandboxes find it.
pub fn pull(home: &Home, id: &str) -> Result<PathBuf> {
    ensure!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'), "invalid system name {id:?}");
    let (base, asset) = (base_url(), asset_name(id));
    let images = home.root().join("images");
    std::fs::create_dir_all(&images)?;

    let sums = agent().get(format!("{base}/SHA256SUMS")).call().context("cannot reach the release")?.into_body().read_to_string()?;
    let expected = sums
        .lines()
        .find_map(|line| line.strip_suffix(&format!("  {asset}")))
        .with_context(|| format!("there is no ready-made image of {id} for this computer ({})", std::env::consts::ARCH))?
        .to_string();

    // Download into a file first: the checksum has to be known before anything is used.
    let archive = images.join(format!(".pull-{id}.tar.gz"));
    let result = (|| -> Result<PathBuf> {
        let response = agent().get(format!("{base}/{asset}")).call().with_context(|| format!("cannot download {asset}"))?;
        let total: Option<u64> = response.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok());
        let mut reader = response.into_body().into_reader();
        let mut file = std::fs::File::create(&archive)?;
        let mut hasher = Sha256::new();
        let (mut buf, mut done, mut shown) = (vec![0u8; 1 << 20], 0u64, 0u64);
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
            done += n as u64;
            // One line per tenth, so a log or the interface can show how far it is.
            if let Some(total) = total.filter(|total| *total > 0) {
                let tenth = done * 10 / total;
                if tenth > shown {
                    shown = tenth;
                    println!("downloading {id}: {}% of {} MB", tenth * 10, total / 1_000_000);
                }
            }
        }
        drop(file);
        let actual: String = hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect();
        if actual != expected {
            bail!("the downloaded image does not match its checksum; nothing was installed");
        }

        println!("unpacking {id}");
        let mut unpacked = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(&archive)?));
        let wanted = |name: &str| ["img", "json", "vmlinuz", "initrd"].iter().any(|ext| name == format!("{id}.{ext}"));
        let mut image = None;
        for entry in unpacked.entries()? {
            let entry = entry?;
            let name = entry.path()?.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            // Only the files of this system, by their plain names: nothing can be written elsewhere.
            if !entry.header().entry_type().is_file() || !wanted(&name) {
                continue;
            }
            let target = images.join(&name);
            let partial = images.join(format!(".{name}.partial"));
            copy_sparse(entry, &mut std::fs::File::create(&partial)?)?;
            std::fs::rename(&partial, &target)?;
            if name.ends_with(".img") {
                image = Some(target);
            }
        }
        image.context("the downloaded archive contains no disk image")
    })();
    let _ = std::fs::remove_file(&archive);
    result
}

/// Make a system's image available: download the ready-made one, and build
/// it on this computer if there is none (or no connection).
pub fn install(home: &Home, id: &str) -> Result<PathBuf> {
    let existing = boxquell_core::os::image(home, id);
    if existing.exists() {
        return Ok(existing);
    }
    match pull(home, id) {
        Ok(image) => Ok(image),
        Err(err) => {
            println!("no ready-made image ({err:#}); building {id} on this computer, which needs Docker and takes a few minutes");
            boxquell_core::os::build(home, id, 4)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_runs_become_holes_and_the_length_is_kept() {
        let path: PathBuf = std::env::temp_dir().join(format!("boxquell-sparse-{}", std::process::id()));
        let mut data = vec![0u8; 3 << 20];
        data[10] = 7;
        data[(2 << 20) + 5] = 9;
        copy_sparse(&data[..], &mut std::fs::File::create(&path).unwrap()).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), data);
        let _ = std::fs::remove_file(&path);
    }
}
