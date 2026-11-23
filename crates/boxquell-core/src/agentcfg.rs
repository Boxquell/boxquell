//! Configuration handed to the agents inside sandboxes: instructions
//! (`CLAUDE.md` / `AGENTS.md`), skills and MCP servers. There is one set for
//! all sandboxes and one per sandbox; a sandbox gets both, its own on top.
//!
//! On disk each set is a plain directory the user may also edit by hand:
//! `instructions.md`, `mcp.json` and `skills/<name>/…`.

use crate::drive::shell_quote;
use crate::Home;
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Everything in one set, as shown to the user.
#[derive(Debug, Default, Serialize)]
pub struct AgentConfig {
    pub instructions: String,
    /// The MCP servers as JSON text: `{ "name": { "command": … } }`. Empty when there are none.
    pub mcp: String,
    pub skills: Vec<String>,
    /// The directory holding this set.
    pub path: PathBuf,
}

/// A file for the guest: path below `agent/` on the config drive, contents, and whether it is executable.
pub type GuestFile = (String, Vec<u8>, bool);

/// Skills are copied into every boot; keep that from getting out of hand.
const MAX_SKILL_BYTES: u64 = 20 * 1024 * 1024;

/// The directory of the set for all sandboxes, or for one.
pub fn dir(home: &Home, sandbox: Option<&str>) -> PathBuf {
    match sandbox {
        Some(name) => home.sandbox(name).join("agent"),
        None => home.root().join("agent"),
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)) && !name.starts_with('.')
}

/// The servers of an MCP configuration. Accepts the bare map of servers as
/// well as the `{ "mcpServers": { … } }` form that agents write themselves.
fn servers(text: &str) -> Result<Map<String, Value>> {
    if text.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value = serde_json::from_str(text).context("the MCP servers are not valid JSON")?;
    let map = match value {
        Value::Object(mut outer) => match outer.remove("mcpServers") {
            Some(Value::Object(inner)) => inner,
            Some(_) => bail!("mcpServers must be an object"),
            None => outer,
        },
        _ => bail!("the MCP servers must be a JSON object of name → server"),
    };
    for (name, server) in &map {
        ensure!(valid_name(name), "invalid MCP server name {name:?}: use letters, digits, '-', '_' and '.'");
        ensure!(server.is_object(), "the MCP server {name:?} must be an object, e.g. {{ \"command\": \"npx\", \"args\": [...] }}");
    }
    Ok(map)
}

pub fn load(dir: &Path) -> AgentConfig {
    let mut skills: Vec<String> = std::fs::read_dir(dir.join("skills"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    skills.sort();
    AgentConfig {
        instructions: std::fs::read_to_string(dir.join("instructions.md")).unwrap_or_default(),
        mcp: std::fs::read_to_string(dir.join("mcp.json")).unwrap_or_default(),
        skills,
        path: dir.to_path_buf(),
    }
}

/// Store instructions and MCP servers. Empty text removes the file.
pub fn save(dir: &Path, instructions: &str, mcp: &str) -> Result<()> {
    let servers = servers(mcp)?;
    std::fs::create_dir_all(dir)?;
    let put = |file: &str, text: String| -> Result<()> {
        let path = dir.join(file);
        if text.trim().is_empty() {
            let _ = std::fs::remove_file(path);
        } else {
            std::fs::write(path, text)?;
        }
        Ok(())
    };
    put("instructions.md", instructions.to_string())?;
    put("mcp.json", if servers.is_empty() { String::new() } else { serde_json::to_string_pretty(&servers)? })
}

fn copy_dir(from: &Path, to: &Path, budget: &mut u64) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_dir(&source, &target, budget)?;
        } else if kind.is_file() {
            let size = entry.metadata()?.len();
            ensure!(size <= *budget, "the skill is larger than {} MiB", MAX_SKILL_BYTES / 1024 / 1024);
            *budget -= size;
            std::fs::copy(&source, &target)?;
        }
        // Links are left out: they would point outside the skill once copied.
    }
    Ok(())
}

/// Copy a skill directory (one that contains `SKILL.md`) into the set. Returns its name.
pub fn add_skill(dir: &Path, source: &Path) -> Result<String> {
    ensure!(source.join("SKILL.md").is_file(), "{} is not a skill: it has no SKILL.md", source.display());
    let name = source.canonicalize()?.file_name().and_then(|n| n.to_str()).map(str::to_string).context("the skill directory has no name")?;
    ensure!(valid_name(&name), "invalid skill name {name:?}: use letters, digits, '-', '_' and '.'");
    let target = dir.join("skills").join(&name);
    let _ = std::fs::remove_dir_all(&target);
    if let Err(err) = copy_dir(source, &target, &mut MAX_SKILL_BYTES.clone()) {
        let _ = std::fs::remove_dir_all(&target);
        return Err(err);
    }
    Ok(name)
}

pub fn remove_skill(dir: &Path, name: &str) -> Result<()> {
    ensure!(valid_name(name), "invalid skill name {name:?}");
    std::fs::remove_dir_all(dir.join("skills").join(name)).with_context(|| format!("no skill named {name:?}"))
}

fn skill_files(dir: &Path, prefix: &str, out: &mut Vec<GuestFile>) {
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(name) = entry.file_name().into_string() else { continue };
        let path = entry.path();
        if path.is_dir() {
            skill_files(&path, &format!("{prefix}/{name}"), out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            #[cfg(unix)]
            let executable = {
                use std::os::unix::fs::PermissionsExt;
                entry.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0)
            };
            #[cfg(not(unix))]
            let executable = false;
            out.push((format!("{prefix}/{name}"), bytes, executable));
        }
    }
}

/// What a sandbox receives: the global set with the sandbox's own on top.
/// Instructions are joined, skills and MCP servers of the sandbox win on equal names.
pub fn guest_files(global: &Path, sandbox: &Path) -> Vec<GuestFile> {
    let (all, own) = (load(global), load(sandbox));
    let mut files = Vec::new();

    let instructions: Vec<&str> = [all.instructions.trim(), own.instructions.trim()].into_iter().filter(|text| !text.is_empty()).collect();
    if !instructions.is_empty() {
        files.push(("instructions.md".to_string(), format!("{}\n", instructions.join("\n\n")).into_bytes(), false));
    }

    // Later entries replace earlier ones of the same path, so the sandbox's skills go last.
    let mut skills = Vec::new();
    skill_files(&global.join("skills"), "skills", &mut skills);
    let own_names: Vec<String> = own.skills.iter().map(|name| format!("skills/{name}/")).collect();
    skills.retain(|(path, _, _)| !own_names.iter().any(|prefix| path.starts_with(prefix)));
    skill_files(&sandbox.join("skills"), "skills", &mut skills);
    files.extend(skills);

    let mut merged = servers(&all.mcp).unwrap_or_default();
    merged.extend(servers(&own.mcp).unwrap_or_default());
    if !merged.is_empty() {
        files.push(("mcp.json".to_string(), serde_json::to_vec_pretty(&serde_json::json!({ "mcpServers": merged })).unwrap_or_default(), false));
        // Claude Code keeps user-level servers in a file of its own; its CLI is the safe way to change it.
        let mut script =
            String::from("#!/bin/bash\n# Registers the MCP servers chosen on the host with Claude Code.\ncommand -v claude >/dev/null || exit 0\n");
        for (name, server) in &merged {
            script += &format!(
                "claude mcp remove --scope user {0} >/dev/null 2>&1\nclaude mcp add-json --scope user {0} {1} >/dev/null 2>&1\n",
                shell_quote(name),
                shell_quote(&server.to_string())
            );
        }
        files.push(("mcp-claude.sh".to_string(), script.into_bytes(), true));
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_gets_the_global_set_with_its_own_on_top() {
        let root = std::env::temp_dir().join(format!("boxquell-agentcfg-{}", std::process::id()));
        let (global, own) = (root.join("global"), root.join("own"));
        save(&global, "Be brief.", r#"{"mcpServers":{"docs":{"command":"a"},"db":{"command":"global"}}}"#).unwrap();
        save(&own, "Use pnpm.", r#"{"db":{"command":"it's own"}}"#).unwrap();
        for (set, skill, text) in [(&global, "review", "global"), (&global, "deploy", "global"), (&own, "deploy", "own")] {
            let source = root.join("src").join(skill);
            std::fs::create_dir_all(&source).unwrap();
            std::fs::write(source.join("SKILL.md"), text).unwrap();
            add_skill(set, &source).unwrap();
        }
        let files = guest_files(&global, &own);
        std::fs::remove_dir_all(&root).unwrap();

        let text =
            |path: &str| String::from_utf8(files.iter().find(|(p, _, _)| p == path).unwrap_or_else(|| panic!("{path} missing")).1.clone()).unwrap();
        assert_eq!(text("instructions.md"), "Be brief.\n\nUse pnpm.\n");
        assert_eq!(text("skills/review/SKILL.md"), "global");
        assert_eq!(text("skills/deploy/SKILL.md"), "own");
        assert_eq!(files.iter().filter(|(p, _, _)| p == "skills/deploy/SKILL.md").count(), 1);
        let mcp: Value = serde_json::from_str(&text("mcp.json")).unwrap();
        assert_eq!(mcp["mcpServers"]["db"]["command"], "it's own");
        assert_eq!(mcp["mcpServers"]["docs"]["command"], "a");
        assert!(text("mcp-claude.sh").contains(r#"add-json --scope user 'db' '{"command":"it'\''s own"}'"#));
    }

    #[test]
    fn bad_input_is_refused() {
        let dir = std::env::temp_dir().join(format!("boxquell-agentcfg-bad-{}", std::process::id()));
        assert!(save(&dir, "", "[1]").is_err());
        assert!(save(&dir, "", r#"{"bad name!":{}}"#).is_err());
        assert!(save(&dir, "", r#"{"x":"not an object"}"#).is_err());
        assert!(add_skill(&dir, Path::new("/nonexistent")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
