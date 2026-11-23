//! Review mode: the sandbox works on a copy of the project folder, and what it
//! changed there is looked at before it reaches the project.
//!
//! Three states of each file are told apart: as it was when the copy was made
//! (the base, kept as a list of fingerprints), as it is in the project now,
//! and as it is in the sandbox's copy. That shows who changed a file: the
//! sandbox, someone in the project, or both.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// A file as it was when the copy was made or last brought in line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    size: u64,
    mtime_ms: u64,
    hash: String,
}

pub type Base = BTreeMap<String, Entry>;

/// Text files beyond this are not compared line by line.
const DIFF_LIMIT: u64 = 1 << 20;
const ROW_LIMIT: usize = 6000;

/// The files of a folder that belong to the project: everything but the
/// repository's own data and what its ignore files leave out (dependencies,
/// build results). Paths are relative, with `/`.
fn files(root: &Path) -> BTreeMap<String, PathBuf> {
    let mut found = BTreeMap::new();
    let walk = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(false)
        .require_git(false)
        .parents(false)
        .follow_links(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walk.flatten() {
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        let Ok(relative) = entry.path().strip_prefix(root) else { continue };
        let name = relative.components().map(|part| part.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
        found.insert(name, entry.into_path());
    }
    found
}

fn stat(file: &Path) -> Option<(u64, u64)> {
    let meta = std::fs::metadata(file).ok()?;
    let mtime = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis() as u64;
    Some((meta.len(), mtime))
}

fn hash(file: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    use std::io::Read;
    let mut reader = std::fs::File::open(file)?;
    let mut chunk = vec![0u8; 1 << 16];
    loop {
        let read = reader.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    Ok(hasher.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn entry(file: &Path) -> Result<Entry> {
    let (size, mtime_ms) = stat(file).with_context(|| format!("cannot read {}", file.display()))?;
    Ok(Entry { size, mtime_ms, hash: hash(file)? })
}

/// Whether a file is still what the base says. Size and time are looked at
/// first; only when those leave it open is the content read.
fn unchanged(file: &Path, base: &Entry) -> bool {
    match stat(file) {
        Some((size, mtime)) if size == base.size && mtime == base.mtime_ms => true,
        Some((size, _)) if size != base.size => false,
        Some(_) => hash(file).is_ok_and(|hash| hash == base.hash),
        None => false,
    }
}

/// Copy one file, keeping its modification time, so the copy counts as unchanged.
fn copy_file(from: &Path, to: &Path) -> Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to).with_context(|| format!("cannot copy {} to {}", from.display(), to.display()))?;
    if let Ok(time) = std::fs::metadata(from).and_then(|meta| meta.modified()) {
        let _ = std::fs::File::options().write(true).open(to).and_then(|file| file.set_modified(time));
    }
    Ok(())
}

/// Make the sandbox's copy of the project and note the base. The whole folder
/// is copied, the repository's data and ignored files included, so tools in
/// the sandbox find what they expect; only project files are compared later.
pub fn start(project: &Path, work: &Path) -> Result<Base> {
    ensure!(project.is_dir(), "the project folder {} is gone", project.display());
    if work.exists() {
        std::fs::remove_dir_all(work)?;
    }
    std::fs::create_dir_all(work)?;
    let fast = cfg!(unix)
        && std::process::Command::new("cp")
            .args(if cfg!(target_os = "macos") { ["-a", "-c"].as_slice() } else { ["-a", "--reflink=auto"].as_slice() })
            .arg(project.join("."))
            .arg(work)
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success());
    if !fast {
        copy_all(project, work)?;
    }
    let mut base = Base::new();
    for (name, file) in files(project) {
        base.insert(name, entry(&file)?);
    }
    Ok(base)
}

fn copy_all(from: &Path, to: &Path) -> Result<()> {
    for item in std::fs::read_dir(from)? {
        let item = item?;
        let (source, target) = (item.path(), to.join(item.file_name()));
        let kind = item.file_type()?;
        if kind.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_all(&source, &target)?;
        } else if kind.is_file() {
            copy_file(&source, &target)?;
        }
    }
    Ok(())
}

fn is_binary(file: &Path) -> bool {
    use std::io::Read;
    let mut start = [0u8; 8192];
    let read = std::fs::File::open(file).and_then(|mut file| file.read(&mut start)).unwrap_or(0);
    start[..read].contains(&0)
}

fn text(file: Option<&PathBuf>) -> String {
    file.and_then(|file| std::fs::read(file).ok()).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default()
}

/// What differs between the project and the sandbox's copy, with who changed
/// it: `sandbox`, `project` (changed in the project since the copy was made)
/// or `both`. Lines added and removed are counted from the project's side
/// towards the sandbox's.
pub fn changes(project: &Path, work: &Path, base: &Base) -> Vec<Value> {
    let (theirs, mine) = (files(project), files(work));
    let names: BTreeSet<&String> = theirs.keys().chain(mine.keys()).chain(base.keys()).collect();
    let mut list = Vec::new();
    for name in names {
        let (original, copy, before) = (theirs.get(name), mine.get(name), base.get(name));
        let differs = |file: Option<&PathBuf>| match (before, file) {
            (None, None) => false,
            (Some(before), Some(file)) => !unchanged(file, before),
            _ => true,
        };
        let (in_sandbox, in_project) = (differs(copy), differs(original));
        if !in_sandbox && !in_project {
            continue;
        }
        // Both ended up the same: nothing to decide.
        let same = match (original, copy) {
            (None, None) => true,
            (Some(a), Some(b)) => stat(a).map(|s| s.0) == stat(b).map(|s| s.0) && hash(a).ok() == hash(b).ok(),
            _ => false,
        };
        if same {
            continue;
        }
        let side = match (in_sandbox, in_project) {
            (true, true) => "both",
            (true, false) => "sandbox",
            _ => "project",
        };
        let kind = match (original, copy) {
            (None, Some(_)) => "added",
            (Some(_), None) => "deleted",
            _ => "modified",
        };
        let binary = original.is_some_and(|file| is_binary(file)) || copy.is_some_and(|file| is_binary(file));
        let large = [original, copy].iter().flatten().any(|file| stat(file).is_some_and(|(size, _)| size > DIFF_LIMIT));
        let (mut added, mut removed) = (0, 0);
        if !binary && !large {
            let (old, new) = (text(original), text(copy));
            for change in similar::TextDiff::from_lines(&old, &new).iter_all_changes() {
                match change.tag() {
                    similar::ChangeTag::Insert => added += 1,
                    similar::ChangeTag::Delete => removed += 1,
                    similar::ChangeTag::Equal => {}
                }
            }
        }
        list.push(json!({ "path": name, "kind": kind, "side": side, "binary": binary, "large": large, "added": added, "removed": removed }));
    }
    list
}

/// A path named by the page, checked to stay inside the folders.
fn relative(name: &str) -> Result<PathBuf> {
    let path = Path::new(name);
    ensure!(!name.is_empty() && path.components().all(|part| matches!(part, Component::Normal(_))), "{name:?} is not a path inside the project");
    Ok(path.to_path_buf())
}

/// The lines of one file's difference, from the project's state to the
/// sandbox's, with a few unchanged lines around each change.
pub fn diff(project: &Path, work: &Path, name: &str, towards_project: bool) -> Result<Value> {
    let path = relative(name)?;
    // For a file that changed in the project, the newer state is the project's.
    let (original, copy) = if towards_project { (work.join(&path), project.join(&path)) } else { (project.join(&path), work.join(&path)) };
    let exists = |file: &PathBuf| file.is_file().then_some(file.clone());
    let (original, copy) = (exists(&original), exists(&copy));
    if original.as_ref().is_some_and(|file| is_binary(file)) || copy.as_ref().is_some_and(|file| is_binary(file)) {
        return Ok(json!({ "binary": true, "rows": [] }));
    }
    if [&original, &copy].iter().filter_map(|file| file.as_ref()).any(|file| stat(file).is_some_and(|(size, _)| size > DIFF_LIMIT)) {
        return Ok(json!({ "large": true, "rows": [] }));
    }
    let (old, new) = (text(original.as_ref()), text(copy.as_ref()));
    let compared = similar::TextDiff::from_lines(&old, &new);
    let mut rows = Vec::new();
    for (index, group) in compared.grouped_ops(3).iter().enumerate() {
        if index > 0 {
            rows.push(json!({ "kind": "gap" }));
        }
        for op in group {
            for change in compared.iter_changes(op) {
                let kind = match change.tag() {
                    similar::ChangeTag::Insert => "add",
                    similar::ChangeTag::Delete => "del",
                    similar::ChangeTag::Equal => "ctx",
                };
                let line = change.value().to_string();
                rows.push(json!({ "kind": kind, "old": change.old_index().map(|n| n + 1), "new": change.new_index().map(|n| n + 1), "text": line.trim_end_matches(['\n', '\r']) }));
                if rows.len() >= ROW_LIMIT {
                    return Ok(json!({ "rows": rows, "cut": true }));
                }
            }
        }
    }
    Ok(json!({ "rows": rows }))
}

/// Bring files of one side in line with the other: `to_project` takes the
/// sandbox's state into the project, otherwise the project's state goes into
/// the sandbox's copy (which discards what the sandbox did to them). The base
/// follows, so the files count as unchanged afterwards.
pub fn settle(project: &Path, work: &Path, base: &mut Base, names: &[String], to_project: bool) -> Result<usize> {
    let mut done = 0;
    for name in names {
        let path = relative(name)?;
        let (source, target) = if to_project { (work.join(&path), project.join(&path)) } else { (project.join(&path), work.join(&path)) };
        if source.is_file() {
            copy_file(&source, &target)?;
            base.insert(name.clone(), entry(&source)?);
        } else if source.exists() {
            bail!("{name} is not a file");
        } else {
            if target.is_file() {
                std::fs::remove_file(&target)?;
            }
            base.remove(name);
        }
        done += 1;
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tells_who_changed_what_and_settles_it() {
        let root = std::env::temp_dir().join(format!("boxquell-review-{}", std::process::id()));
        let (project, work) = (root.join("project"), root.join("work"));
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::create_dir_all(project.join("node_modules/x")).unwrap();
        let put = |dir: &Path, name: &str, content: &str| std::fs::write(dir.join(name), content).unwrap();
        put(&project, ".gitignore", "node_modules\n");
        put(&project, "src/a.txt", "one\ntwo\nthree\n");
        put(&project, "src/b.txt", "keep\n");
        put(&project, "src/c.txt", "mine\n");
        put(&project, "node_modules/x/big.js", "ignored\n");
        let mut base = start(&project, &work).unwrap();
        assert!(work.join("node_modules/x/big.js").exists(), "the copy holds ignored files too");
        assert!(changes(&project, &work, &base).is_empty());

        // The sandbox edits one file, adds one, deletes one, and touches an ignored one.
        put(&work, "src/a.txt", "one\n2\nthree\nfour\n");
        put(&work, "src/new.txt", "fresh\n");
        std::fs::remove_file(work.join("src/b.txt")).unwrap();
        put(&work, "node_modules/x/big.js", "changed\n");
        // Meanwhile someone edits another file in the project.
        put(&project, "src/c.txt", "theirs now\n");

        let found = changes(&project, &work, &base);
        let of = |path: &str| found.iter().find(|change| change["path"] == path).unwrap_or_else(|| panic!("{path} is missing"));
        assert_eq!(found.len(), 4);
        assert_eq!(
            (
                of("src/a.txt")["kind"].as_str(),
                of("src/a.txt")["side"].as_str(),
                of("src/a.txt")["added"].as_u64(),
                of("src/a.txt")["removed"].as_u64()
            ),
            (Some("modified"), Some("sandbox"), Some(2), Some(1))
        );
        assert_eq!((of("src/new.txt")["kind"].as_str(), of("src/b.txt")["kind"].as_str()), (Some("added"), Some("deleted")));
        assert_eq!(of("src/c.txt")["side"], "project");

        let rows = diff(&project, &work, "src/a.txt", false).unwrap();
        let kinds: Vec<&str> = rows["rows"].as_array().unwrap().iter().map(|row| row["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["ctx", "del", "add", "ctx", "add"]);
        assert!(diff(&project, &work, "../outside", false).is_err());

        // Take the sandbox's edit and its new file, discard its deletion, and bring the project's edit in.
        settle(&project, &work, &mut base, &["src/a.txt".into(), "src/new.txt".into()], true).unwrap();
        settle(&project, &work, &mut base, &["src/b.txt".into(), "src/c.txt".into()], false).unwrap();
        assert_eq!(std::fs::read_to_string(project.join("src/a.txt")).unwrap(), "one\n2\nthree\nfour\n");
        assert_eq!(std::fs::read_to_string(work.join("src/b.txt")).unwrap(), "keep\n");
        assert_eq!(std::fs::read_to_string(work.join("src/c.txt")).unwrap(), "theirs now\n");
        assert!(changes(&project, &work, &base).is_empty());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
