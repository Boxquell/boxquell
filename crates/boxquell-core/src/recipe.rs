//! Recipes: what can be installed into a sandbox (agents, languages, tools)
//! and which network destinations and secrets each one needs.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Agent,
    Language,
    Tool,
    Ide,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recipe {
    /// Taken from the file name; recipes from a repository are `<repository>:<file name>`.
    #[serde(default)]
    pub id: String,
    /// Where the recipe comes from: `builtin`, `local` or the repository's name.
    #[serde(default)]
    pub source: String,
    pub name: String,
    pub category: Category,
    pub description: String,
    /// Versions on offer, the default first. The install script gets the chosen
    /// one as `BOXQUELL_VERSION`; select it with `<recipe>@<version>`.
    #[serde(default)]
    pub versions: Vec<String>,
    /// Recipes that must be installed first.
    #[serde(default)]
    pub requires: Vec<String>,
    /// Destinations allowed only while the install script runs.
    #[serde(default)]
    pub install_allow: Vec<String>,
    /// Destinations the installed software needs to work.
    #[serde(default)]
    pub allow: Vec<String>,
    /// Host secrets this software can use; those that are set get attached.
    #[serde(default)]
    pub secrets: Vec<String>,
    /// For agents: the command that starts it.
    #[serde(default)]
    pub command: Option<String>,
    /// Environment variables for sessions in the guest.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Shell script run in the guest as the `dev` user (with passwordless sudo).
    pub install: String,
}

macro_rules! builtin {
    ($($id:literal),*) => { [$(($id, include_str!(concat!("../../../recipes/", $id, ".toml")))),*] };
}
const BUILTIN: [(&str, &str); 10] = builtin!("claude", "codex", "opencode", "pi", "node", "python", "go", "rust", "build-essential", "gh");

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn parse(id: &str, source: &str, text: &str) -> Result<Recipe> {
    let mut recipe: Recipe = toml::from_str(text).with_context(|| format!("recipe {id} is invalid"))?;
    recipe.id = id.to_string();
    recipe.source = source.to_string();
    Ok(recipe)
}

/// The `*.toml` recipe files in `dir` as (file stem, path).
fn recipe_files(dir: &Path) -> Vec<(String, std::path::PathBuf)> {
    let mut files: Vec<_> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .filter_map(|path| Some((path.file_stem()?.to_str()?.to_string(), path)))
        .filter(|(stem, _)| valid_id(stem))
        .collect();
    files.sort();
    files
}

/// Recipes of a repository checkout live in its `recipes/` directory, or at its top level.
pub fn repo_recipe_dir(checkout: &Path) -> std::path::PathBuf {
    let sub = checkout.join("recipes");
    if sub.is_dir() {
        sub
    } else {
        checkout.to_path_buf()
    }
}

/// A recipe to install together with the chosen version, if it has versions.
#[derive(Debug, Clone)]
pub struct Pick<'a> {
    pub recipe: &'a Recipe,
    pub version: Option<String>,
}

impl Pick<'_> {
    /// `node@22`, or just the id for recipes without versions.
    pub fn label(&self) -> String {
        match &self.version {
            Some(version) => format!("{}@{version}", self.recipe.id),
            None => self.recipe.id.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Catalog(BTreeMap<String, Recipe>);

impl Catalog {
    /// Built-in recipes, the checkouts in `repos_dir` (ids prefixed with the
    /// repository name, so they can never replace a built-in), and `*.toml`
    /// files in `user_dir`, which override built-ins of the same id.
    /// A broken file in a repository is skipped; a broken local file is an error.
    pub fn load(user_dir: &Path, repos_dir: &Path) -> Result<Self> {
        let mut all = BTreeMap::new();
        for (id, text) in BUILTIN {
            all.insert(id.to_string(), parse(id, "builtin", text)?);
        }
        let mut repos: Vec<_> = std::fs::read_dir(repos_dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        repos.sort();
        for checkout in repos {
            let Some(repo) = checkout.file_name().and_then(|n| n.to_str()).filter(|n| valid_id(n)) else { continue };
            for (stem, path) in recipe_files(&repo_recipe_dir(&checkout)) {
                let id = format!("{repo}:{stem}");
                if let Ok(mut recipe) = std::fs::read_to_string(&path).map_err(anyhow::Error::from).and_then(|text| parse(&id, repo, &text)) {
                    // Dependencies name recipes of the same repository first, then built-ins.
                    for dep in &mut recipe.requires {
                        if !dep.contains(':') && recipe_files(&repo_recipe_dir(&checkout)).iter().any(|(s, _)| s == dep) {
                            *dep = format!("{repo}:{dep}");
                        }
                    }
                    all.insert(id, recipe);
                }
            }
        }
        for (id, path) in recipe_files(user_dir) {
            all.insert(id.clone(), parse(&id, "local", &std::fs::read_to_string(&path)?).with_context(|| path.display().to_string())?);
        }
        Ok(Self(all))
    }

    pub fn of(home: &crate::Home) -> Result<Self> {
        Self::load(&home.recipes(), &home.repos())
    }

    /// Look up `id`. A repository recipe may be named without its prefix when that is unambiguous.
    pub fn find(&self, id: &str) -> Result<&Recipe> {
        if let Some(recipe) = self.0.get(id) {
            return Ok(recipe);
        }
        let matches: Vec<&Recipe> = self.0.values().filter(|r| r.id.split_once(':').is_some_and(|(_, stem)| stem == id)).collect();
        match matches.as_slice() {
            [one] => Ok(one),
            [] => bail!("unknown recipe {id:?}; see `boxquell recipes`"),
            many => bail!("{id:?} exists in several repositories; use one of: {}", many.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join(", ")),
        }
    }

    pub fn get(&self, id: &str) -> Option<&Recipe> {
        self.0.get(id)
    }

    /// All recipes, agents first.
    pub fn all(&self) -> Vec<&Recipe> {
        let mut list: Vec<&Recipe> = self.0.values().collect();
        list.sort_by(|a, b| (a.category, &a.id).cmp(&(b.category, &b.id)));
        list
    }

    /// `ids` and everything they require, each once, dependencies first.
    pub fn resolve(&self, ids: &[String]) -> Result<Vec<&Recipe>> {
        Ok(self.resolve_versions(ids)?.into_iter().map(|pick| pick.recipe).collect())
    }

    /// Like [`resolve`](Self::resolve), with the version of each recipe: the one
    /// asked for as `<recipe>@<version>`, otherwise the recipe's default.
    pub fn resolve_versions(&self, ids: &[String]) -> Result<Vec<Pick<'_>>> {
        let mut wanted = BTreeMap::new();
        let mut plain = Vec::new();
        for id in ids {
            let (id, version) = match id.split_once('@') {
                Some((id, version)) => (id, Some(version)),
                None => (id.as_str(), None),
            };
            let recipe = self.find(id)?;
            if let Some(version) = version {
                if !recipe.versions.iter().any(|v| v == version) {
                    let offer = if recipe.versions.is_empty() {
                        "it has no versions to choose from".to_string()
                    } else {
                        format!("choose one of: {}", recipe.versions.join(", "))
                    };
                    bail!("{} has no version {version:?}; {offer}", recipe.id);
                }
                wanted.insert(recipe.id.clone(), version.to_string());
            }
            plain.push(recipe.id.clone());
        }
        Ok(self
            .resolve_plain(&plain)?
            .into_iter()
            .map(|recipe| Pick { recipe, version: wanted.remove(&recipe.id).or_else(|| recipe.versions.first().cloned()) })
            .collect())
    }

    fn resolve_plain(&self, ids: &[String]) -> Result<Vec<&Recipe>> {
        fn visit<'a>(catalog: &'a Catalog, id: &str, path: &mut Vec<String>, out: &mut Vec<&'a Recipe>) -> Result<()> {
            if out.iter().any(|r| r.id == id) {
                return Ok(());
            }
            if path.iter().any(|p| p == id) {
                bail!("recipes require each other in a circle: {} -> {id}", path.join(" -> "));
            }
            let recipe = catalog.find(id)?;
            let id = recipe.id.as_str();
            if out.iter().any(|r| r.id == id) {
                return Ok(());
            }
            path.push(id.to_string());
            for dep in &recipe.requires {
                visit(catalog, dep, path, out)?;
            }
            path.pop();
            out.push(recipe);
            Ok(())
        }
        let mut out = Vec::new();
        for id in ids {
            visit(self, id, &mut Vec::new(), &mut out)?;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtins_parse_and_resolve_dependencies_first() {
        let catalog = Catalog::load(Path::new("/nonexistent"), Path::new("/nonexistent")).unwrap();
        for recipe in catalog.all() {
            catalog.resolve(std::slice::from_ref(&recipe.id)).unwrap();
        }
        let order: Vec<&str> = catalog.resolve(&["claude".into(), "codex".into()]).unwrap().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(order, ["node", "claude", "codex"]);
        assert!(catalog.resolve(&["nope".into()]).is_err());

        let labels = |ids: &[&str]| -> Vec<String> {
            let ids: Vec<String> = ids.iter().map(|id| id.to_string()).collect();
            catalog.resolve_versions(&ids).unwrap().iter().map(Pick::label).collect()
        };
        assert_eq!(labels(&["claude", "node@22"]), ["node@22", "claude"]);
        assert_eq!(labels(&["node"])[0], format!("node@{}", catalog.find("node").unwrap().versions[0]));
        assert!(catalog.resolve_versions(&["node@1".into()]).is_err());
        assert!(catalog.resolve_versions(&["claude@1".into()]).is_err());
    }
}

#[cfg(test)]
mod repo_tests {
    use super::*;

    #[test]
    fn repository_recipes_are_prefixed_and_cannot_replace_builtins() {
        let dir = std::env::temp_dir().join(format!("boxquell-recipe-test-{}", std::process::id()));
        let repo = dir.join("extra").join("recipes");
        std::fs::create_dir_all(&repo).unwrap();
        let recipe = |requires: &str| format!("name='X'\ncategory='tool'\ndescription='d'\nrequires=[{requires}]\ninstall='true'\n");
        std::fs::write(repo.join("claude.toml"), recipe("")).unwrap();
        std::fs::write(repo.join("base.toml"), recipe("'node'")).unwrap();
        std::fs::write(repo.join("zed.toml"), recipe("'base'")).unwrap();
        std::fs::write(repo.join("broken.toml"), "nope").unwrap();
        let catalog = Catalog::load(Path::new("/nonexistent"), &dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(catalog.find("claude").unwrap().source, "builtin");
        assert_eq!(catalog.find("extra:claude").unwrap().source, "extra");
        assert!(catalog.find("extra:broken").is_err());
        let order: Vec<&str> = catalog.resolve(&["zed".into()]).unwrap().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(order, ["node", "extra:base", "extra:zed"]);
    }
}
