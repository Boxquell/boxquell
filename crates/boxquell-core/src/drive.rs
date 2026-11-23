//! The config drive: a plain tar archive attached to the guest as a read-only
//! disk. `boxquell-init` unpacks it at boot. A tar on a block device needs no
//! filesystem tooling on the host and works the same with every VM backend.

use anyhow::Result;
use std::path::Path;

pub struct DriveFile<'a> {
    pub path: &'a str,
    pub contents: &'a [u8],
    pub mode: u32,
}

pub fn write(path: &Path, files: &[DriveFile]) -> Result<()> {
    let mut archive = tar::Builder::new(Vec::new());
    for file in files {
        let mut header = tar::Header::new_ustar();
        header.set_size(file.contents.len() as u64);
        header.set_mode(file.mode);
        header.set_cksum();
        archive.append_data(&mut header, file.path, file.contents)?;
    }
    let mut bytes = archive.into_inner()?;
    // Block devices are sized in whole sectors; pad generously so every backend accepts it.
    bytes.resize(bytes.len().next_multiple_of(64 * 1024), 0);
    crate::write_private(path, &bytes)?;
    Ok(())
}

/// Quote a value for a POSIX shell `export` line.
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's $HOME"), r"'it'\''s $HOME'");
    }

    #[test]
    fn archive_round_trip() {
        let dir = std::env::temp_dir().join(format!("boxquell-drive-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.tar");
        write(&path, &[DriveFile { path: "env.sh", contents: b"export A='1'\n", mode: 0o644 }]).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(bytes.len() % 512, 0);
        let mut archive = tar::Archive::new(&bytes[..]);
        let names: Vec<_> = archive.entries().unwrap().map(|e| e.unwrap().path().unwrap().display().to_string()).collect();
        assert_eq!(names, ["env.sh"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
