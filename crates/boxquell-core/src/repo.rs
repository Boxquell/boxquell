//! Recipe repositories: git repositories with recipe files, cloned to the
//! host. Anyone can publish one; `boxquell repo add <url>` makes its recipes
//! installable as `<repository>:<recipe>`.

use crate::recipe::repo_recipe_dir;
use crate::Home;
use anyhow::{bail, ensure, Context, Result};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Serialize)]
pub struct Repo {
    pub name: String,
    pub url: String,
    /// Short commit id of the checkout.
    pub revision: String,
    pub recipes: usize,
}

fn valid_name(name: &str) -> bool {
    (1..=32).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && !name.starts_with('-')
}

/// `https://example.com/me/boxquell-recipes.git` → `recipes`; `…/My_Tools` → `my-tools`.
pub fn name_from_url(url: &str) -> String {
    let last = url.trim_end_matches('/').rsplit(['/', ':']).next().unwrap_or_default();
    let stem = last.strip_suffix(".git").unwrap_or(last).to_ascii_lowercase();
    let clean: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let clean = clean.trim_matches('-');
    clean.strip_prefix("boxquell-").filter(|rest| !rest.is_empty()).unwrap_or(clean).to_string()
}

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    // Never stop to ask for a password: a repository that needs one fails with a message instead.
    let output = command.args(args).env("GIT_TERMINAL_PROMPT", "0").output().context("cannot run git; is it installed?")?;
    if !output.status.success() {
        bail!("git {}: {}", args.first().unwrap_or(&""), String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn describe(checkout: &Path) -> Option<Repo> {
    let name = checkout.file_name()?.to_str()?.to_string();
    let count = std::fs::read_dir(repo_recipe_dir(checkout)).ok()?.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "toml")).count();
    Some(Repo {
        url: git(Some(checkout), &["config", "--get", "remote.origin.url"]).unwrap_or_default(),
        revision: git(Some(checkout), &["rev-parse", "--short", "HEAD"]).unwrap_or_default(),
        recipes: count,
        name,
    })
}

pub fn list(home: &Home) -> Vec<Repo> {
    let mut dirs: Vec<_> =
        std::fs::read_dir(home.repos()).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.join(".git").exists()).collect();
    dirs.sort();
    dirs.iter().filter_map(|dir| describe(dir)).collect()
}

/// Clone `url`. The name defaults to the last part of the URL.
pub fn add(home: &Home, url: &str, name: Option<&str>) -> Result<Repo> {
    let url = url.trim();
    // A leading dash would be read by git as an option.
    ensure!(!url.is_empty() && !url.starts_with('-'), "give the repository's URL");
    let name = name.map(str::to_string).unwrap_or_else(|| name_from_url(url));
    ensure!(valid_name(&name), "invalid repository name {name:?}: use 1-32 characters of a-z, 0-9 and '-' (set one with --name)");
    ensure!(!["builtin", "local"].contains(&name.as_str()), "the name {name:?} is reserved");
    let dir = home.repos().join(&name);
    ensure!(!dir.exists(), "a repository named {name:?} is already added; `boxquell repo update {name}` fetches its latest recipes");
    std::fs::create_dir_all(home.repos())?;
    let partial = home.repos().join(format!(".{name}.partial"));
    let _ = std::fs::remove_dir_all(&partial);
    let cloned = git(None, &["clone", "--quiet", "--depth", "1", "--", url, &partial.to_string_lossy()]);
    if let Err(err) = cloned {
        let _ = std::fs::remove_dir_all(&partial);
        return Err(err);
    }
    let repo = describe(&partial).filter(|repo| repo.recipes > 0);
    if repo.is_none() {
        let _ = std::fs::remove_dir_all(&partial);
        bail!("{url} has no recipes: expected *.toml files in a `recipes` directory or at the top level");
    }
    std::fs::rename(&partial, &dir)?;
    describe(&dir).context("the new checkout cannot be read")
}

fn checkout(home: &Home, name: &str) -> Result<std::path::PathBuf> {
    ensure!(valid_name(name), "invalid repository name {name:?}");
    let dir = home.repos().join(name);
    ensure!(dir.join(".git").exists(), "no repository named {name:?}; see `boxquell repo ls`");
    Ok(dir)
}

/// Fetch the latest recipes of one repository.
pub fn update(home: &Home, name: &str) -> Result<Repo> {
    let dir = checkout(home, name)?;
    git(Some(&dir), &["fetch", "--quiet", "--depth", "1", "origin", "HEAD"])?;
    git(Some(&dir), &["reset", "--quiet", "--hard", "FETCH_HEAD"])?;
    describe(&dir).context("the checkout cannot be read")
}

/// Forget a repository. Software already installed from it stays in its sandboxes.
pub fn remove(home: &Home, name: &str) -> Result<()> {
    Ok(std::fs::remove_dir_all(checkout(home, name)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_come_from_the_last_url_part() {
        assert_eq!(name_from_url("https://example.com/me/boxquell-recipes.git"), "recipes");
        assert_eq!(name_from_url("git@example.com:me/My_Tools/"), "my-tools");
        assert_eq!(name_from_url("/srv/git/boxquell-"), "boxquell");
    }
}
