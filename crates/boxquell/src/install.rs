//! Install recipes into a sandbox: open the network for the install, run the
//! script in the guest over ssh, then close what only the install needed.

use crate::client::Client;
use crate::ssh;
use anyhow::{bail, Context, Result};
use boxquell_core::drive::shell_quote;
use boxquell_core::recipe::{Catalog, Pick};
use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const MARKERS: &str = "/var/lib/boxquell/recipes";

/// The script sent to the guest for one recipe. Exit code 0 means installed (now or earlier).
fn guest_script(pick: &Pick, again: bool) -> String {
    let recipe = pick.recipe;
    // The marker carries the version, so asking for another one installs it.
    let marker = format!("{MARKERS}/{}", pick.label());
    let mut script = String::from("set -euo pipefail\n");
    if !again {
        script += &format!("if [ -e {marker} ]; then echo 'already installed'; exit 0; fi\n");
    }
    if let Some(version) = &pick.version {
        script += &format!("export BOXQUELL_VERSION={}\n", shell_quote(version));
    }
    if !recipe.env.is_empty() {
        // Debian reads only profile files whose names are letters, digits, `.`, `_` and `-`.
        let profile = format!("/etc/profile.d/boxquell-recipe-{}.sh", recipe.id.replace(':', "-"));
        let exports: String = recipe.env.iter().map(|(k, v)| format!("export {k}={}\n", shell_quote(v))).collect();
        script += &format!("printf %s {} | sudo tee {profile} >/dev/null\n. {profile}\n", shell_quote(&exports));
    }
    script += &format!("(\n{}\n)\nsudo mkdir -p {MARKERS}\nsudo rm -f {MARKERS}/{}@*\nsudo touch {marker}\n", recipe.install.trim(), recipe.id);
    script
}

fn rules_of(info: &Value) -> Vec<String> {
    info["rules"].as_array().into_iter().flatten().filter_map(|r| r.as_str().map(str::to_string)).collect()
}

/// Take software out of a sandbox: run the recipe's uninstall script where it
/// has one, then forget the recipe. What other installed software requires stays.
pub fn uninstall(client: &Client, sandbox: &str, catalog: &Catalog, ids: &[String]) -> Result<()> {
    let path = format!("/api/sandboxes/{sandbox}");
    let before = client.get(&path)?;
    let installed: Vec<String> =
        before["recipes"].as_array().into_iter().flatten().filter_map(|r| r.as_str()).map(|r| r.split('@').next().unwrap_or(r).to_string()).collect();
    let mut going = Vec::new();
    for id in ids {
        let recipe = catalog.find(id.split('@').next().unwrap_or(id))?;
        if !installed.contains(&recipe.id) {
            bail!("{} is not installed in {sandbox}", recipe.name);
        }
        going.push(recipe);
    }
    // Something that stays must not lose what it requires.
    for other in installed.iter().filter(|id| !going.iter().any(|r| r.id == **id)) {
        let Some(other) = catalog.get(other) else { continue };
        for dep in &other.requires {
            let needed = catalog.find(boxquell_core::recipe::requirement(dep).0)?;
            if going.iter().any(|r| r.id == needed.id) {
                bail!("{} is needed by {}; remove that first, or both together", needed.name, other.name);
            }
        }
    }
    // What requires another goes first.
    going.sort_by_key(|recipe| std::cmp::Reverse(installed.iter().position(|id| *id == recipe.id)));
    let with_script = going.iter().any(|recipe| recipe.uninstall.is_some());
    let (key, port) = if with_script {
        let info = client.post(&format!("{path}/start"), json!({}))?;
        (
            PathBuf::from(info["ssh_key"].as_str().context("daemon reported no ssh key")?),
            info["ssh_port"].as_u64().context("daemon reported no ssh port")? as u16,
        )
    } else {
        (PathBuf::new(), 0)
    };
    for recipe in going {
        println!("==> removing {}", recipe.name);
        let forget = format!(
            "sudo rm -f {MARKERS}/{id} {MARKERS}/{id}@* /etc/profile.d/boxquell-recipe-{file}.sh\n",
            id = recipe.id,
            file = recipe.id.replace(':', "-")
        );
        match &recipe.uninstall {
            Some(script) => run_script(&key, port, &format!("set -euo pipefail\n(\n{}\n)\n{forget}", script.trim()))
                .with_context(|| format!("removing {} failed; it is still listed as installed", recipe.id))?,
            None => {
                println!("    the recipe has no uninstall script: it is taken off the list, its files stay in the sandbox");
                if before["running"] == true {
                    let info = client.get(&path)?;
                    let key = PathBuf::from(info["ssh_key"].as_str().unwrap_or_default());
                    let _ = run_script(&key, info["ssh_port"].as_u64().unwrap_or(0) as u16, &forget);
                }
            }
        }
        client.post(&format!("{path}/forgotten"), json!({ "recipes": [recipe.id] }))?;
    }
    Ok(())
}

pub fn install(client: &Client, sandbox: &str, catalog: &Catalog, ids: &[String], again: bool) -> Result<()> {
    let picks = catalog.resolve_versions(ids)?;
    // Running it again concerns what was named, not what that requires.
    let named: Vec<String> =
        ids.iter().filter_map(|id| catalog.find(id.split('@').next().unwrap_or(id)).ok().map(|recipe| recipe.id.clone())).collect();
    let recipes: Vec<_> = picks.iter().map(|pick| pick.recipe).collect();
    let path = format!("/api/sandboxes/{sandbox}");

    // Install scripts are written for one kind of system; say so before anything runs.
    let system = client.get(&path)?;
    let family = system["family"].as_str().unwrap_or(boxquell_core::os::DEFAULT_FAMILY).to_string();
    // `@packages` in a recipe stands for the hosts of this system's package manager.
    let package_hosts: Vec<String> =
        system["package_hosts"].as_array().into_iter().flatten().filter_map(|host| host.as_str().map(str::to_string)).collect();
    let expand = |hosts: &[String]| -> Vec<String> {
        hosts.iter().flat_map(|host| if host == "@packages" { package_hosts.clone() } else { vec![host.clone()] }).collect()
    };
    for recipe in &recipes {
        let supported: Vec<&str> =
            if recipe.os.is_empty() { vec![boxquell_core::os::DEFAULT_FAMILY] } else { recipe.os.iter().map(String::as_str).collect() };
        if !supported.contains(&family.as_str()) {
            bail!("{} is written for {} systems; {sandbox} runs {family}", recipe.name, supported.join(" and "));
        }
    }

    // Secrets first: a sandbox that is not running yet starts with their placeholders.
    let stored: Vec<String> =
        client.get("/api/secrets")?.as_array().into_iter().flatten().filter_map(|s| s["name"].as_str().map(str::to_string)).collect();
    let before = client.get(&path)?;
    let was_running = before["running"] == true;
    let mut attached = Vec::new();
    for secret in recipes.iter().flat_map(|r| &r.secrets) {
        let known = before["secrets"].as_array().is_some_and(|list| list.iter().any(|s| s == secret.as_str()));
        if stored.contains(secret) && !known && !attached.contains(secret) {
            client.post(&format!("{path}/secrets"), json!({ "secret": secret }))?;
            attached.push(secret.clone());
        }
    }

    let info = client.post(&format!("{path}/start"), json!({}))?;
    let key = PathBuf::from(info["ssh_key"].as_str().context("daemon reported no ssh key")?);
    let port = info["ssh_port"].as_u64().context("daemon reported no ssh port")? as u16;

    for pick in &picks {
        let recipe = pick.recipe;
        println!("==> {} ({})", recipe.name, pick.label());
        if !recipe.allow.is_empty() {
            println!("    opens for this sandbox: {} (switch off under Network, or with `boxquell deny`)", expand(&recipe.allow).join(", "));
        }
        if !recipe.official && !recipe.install_allow.is_empty() {
            println!("    from {}: opens while installing: {}", recipe.source, expand(&recipe.install_allow).join(", "));
        }
        let existing = rules_of(&client.get(&path)?);
        let add = |host: &String| client.post(&format!("{path}/rules"), json!({ "rule": format!("allow {host}") })).map(drop);
        let mut temporary = Vec::new();
        let (install_allow, allow) = (expand(&recipe.install_allow), expand(&recipe.allow));
        for host in &install_allow {
            let rule = format!("allow {host}");
            if !existing.contains(&rule) {
                add(host)?;
                temporary.push(rule);
            }
        }
        // What the software needs later opens by itself once it is recorded as
        // installed, and can be switched off there. Until then it is open like the rest.
        for host in &allow {
            let rule = format!("allow {host}");
            if !existing.contains(&rule) && !temporary.contains(&rule) {
                add(host)?;
                temporary.push(rule);
            }
        }

        let outcome = run_script(&key, port, &guest_script(pick, again && named.contains(&recipe.id)));
        for rule in temporary {
            client.delete(&format!("{path}/rules"), Some(json!({ "rule": rule })))?;
        }
        outcome.with_context(|| format!("installing {} failed; `boxquell logs {sandbox} --blocked` shows refused connections", recipe.id))?;
        // Noted as installed at once: what it opens is then open for the recipes that follow and need it.
        client.post(&format!("{path}/installed"), json!({ "recipes": [pick.label()] }))?;
    }

    // Ports the installed software listens on become reachable from this computer.
    for port in recipes.iter().flat_map(|r| &r.ports) {
        match client.post(&format!("{path}/ports"), json!({ "host": port, "guest": port })) {
            Ok(_) => println!("forwarded http://127.0.0.1:{port} to port {port} in the sandbox"),
            Err(err) if format!("{err:#}").contains("already leads") => {}
            Err(err) => println!("note: port {port} is not forwarded ({err:#}); choose another with `boxquell port add {sandbox} <port>:{port}`"),
        }
    }
    let installed: Vec<String> = picks.iter().map(Pick::label).collect();
    client.post(&format!("{path}/installed"), json!({ "recipes": installed }))?;

    for recipe in recipes.iter().filter(|r| !r.secrets.is_empty()) {
        if !recipe.secrets.iter().any(|s| stored.contains(s)) {
            println!(
                "note: {} needs a credential: run `boxquell secret set <name>` with one of [{}], then `boxquell add {sandbox} {}` again",
                recipe.name,
                recipe.secrets.join(", "),
                recipe.id
            );
        }
    }
    if !attached.is_empty() && was_running {
        println!("note: attached {}; restart the sandbox (`boxquell down {sandbox}`) to get the placeholder variables", attached.join(", "));
    }
    Ok(())
}

fn run_script(key: &std::path::Path, port: u16, script: &str) -> Result<()> {
    let mut child = Command::new("ssh")
        .args(ssh::base_args(key, port))
        // The login shell loads the sandbox environment; the script runs in a plain
        // child shell so that ~/.bash_logout cannot change its exit status.
        .arg("bash -lc 'exec bash -s'")
        .stdin(Stdio::piped())
        .spawn()
        .context("cannot run ssh; is OpenSSH installed?")?;
    child.stdin.take().context("ssh has no stdin")?.write_all(script.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        bail!("the install script ended with {status}");
    }
    Ok(())
}
