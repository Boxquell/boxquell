//! Claude Code settings for a sandbox, typically pointing at an LLM gateway
//! such as LiteLLM. The file is handed to the guest as its managed settings,
//! after every credential in it has been swapped for a placeholder: the real
//! value becomes a secret of that sandbox and never enters the guest.

use crate::SecretDef;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::PathBuf;

/// The address under which a guest reaches the computer it runs on.
const HOST_FROM_GUEST: &str = "10.0.2.2";
const DEFAULT_API_HOST: &str = "api.anthropic.com";
/// Settings whose values are credentials, with the name of the secret they become.
const CREDENTIALS: [(&str, &str); 2] = [("ANTHROPIC_AUTH_TOKEN", "claude-auth-token"), ("ANTHROPIC_API_KEY", "claude-api-key")];

#[derive(Debug)]
pub struct Imported {
    /// The settings file for the guest; it holds placeholders only.
    pub settings: String,
    pub secrets: Vec<SecretDef>,
    /// Network rules the settings need, e.g. `allow llm.example.com:4000`.
    pub rules: Vec<String>,
    /// Things the user should know about what was changed.
    pub notes: Vec<String>,
}

/// This computer's managed settings as one document: the base file with the
/// drop-in files of `managed-settings.d` on top, in alphabetical order. Later
/// files win for single values, objects are merged and lists are joined.
pub fn host_managed_text() -> Result<String> {
    let base = host_managed_settings();
    let mut merged = Value::Object(Default::default());
    let mut found = false;
    let mut files = vec![base.clone()];
    let mut drop_ins: Vec<PathBuf> = std::fs::read_dir(base.with_file_name("managed-settings.d"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json") && !path.file_name().is_some_and(|name| name.to_string_lossy().starts_with('.')))
        .collect();
    drop_ins.sort();
    files.extend(drop_ins);
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else { continue };
        let value: Value = serde_json::from_str(&text).with_context(|| format!("{} is not valid JSON", file.display()))?;
        merge(&mut merged, value);
        found = true;
    }
    anyhow::ensure!(found, "this computer has no managed Claude Code settings ({})", base.display());
    Ok(serde_json::to_string_pretty(&merged)?)
}

fn merge(into: &mut Value, from: Value) {
    match (into, from) {
        (Value::Object(into), Value::Object(from)) => {
            for (key, value) in from {
                merge(into.entry(key).or_insert(Value::Null), value);
            }
        }
        (Value::Array(into), Value::Array(from)) => {
            for item in from {
                if !into.contains(&item) {
                    into.push(item);
                }
            }
        }
        (into, from) => *into = from,
    }
}

/// Where Claude Code looks for managed settings on this computer.
pub fn host_managed_settings() -> PathBuf {
    if cfg!(target_os = "macos") {
        "/Library/Application Support/ClaudeCode/managed-settings.json".into()
    } else if cfg!(windows) {
        r"C:\Program Files\ClaudeCode\managed-settings.json".into()
    } else {
        "/etc/claude-code/managed-settings.json".into()
    }
}

struct Endpoint {
    scheme: String,
    host: String,
    port: u16,
    path: String,
}

fn parse_url(url: &str) -> Result<Endpoint> {
    let (scheme, rest) = url.split_once("://").with_context(|| format!("ANTHROPIC_BASE_URL {url:?} is not a URL"))?;
    let default_port = match scheme {
        "http" => 80,
        "https" => 443,
        _ => bail!("ANTHROPIC_BASE_URL {url:?} must start with http:// or https://"),
    };
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, ""),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse().with_context(|| format!("invalid port in {url:?}"))?),
        None => (authority, default_port),
    };
    if host.is_empty() || host.contains(['@', '[']) {
        bail!("ANTHROPIC_BASE_URL {url:?}: expected a plain host name or IPv4 address");
    }
    Ok(Endpoint { scheme: scheme.into(), host: host.to_ascii_lowercase(), port, path: path.into() })
}

pub fn import(text: &str) -> Result<Imported> {
    let mut settings: Value = serde_json::from_str(text).context("the settings are not valid JSON")?;
    let root = settings.as_object_mut().context("the settings must be a JSON object")?;
    let mut notes = Vec::new();
    let mut rules = Vec::new();
    let mut secrets = Vec::new();

    // Programs on this computer that the sandbox does not have. The key helper
    // is handled below: it keeps running here. The others cannot follow.
    let key_helper = root.remove("apiKeyHelper").and_then(|helper| helper.as_str().map(str::to_string)).filter(|helper| !helper.trim().is_empty());
    for key in [
        "awsAuthRefresh",
        "awsCredentialExport",
        "gcpAuthRefresh",
        "otelHeadersHelper",
        "policyHelper",
        "processWrapper",
        "fileSuggestion",
        "statusLine",
    ] {
        if root.remove(key).is_some() {
            notes.push(format!("removed {key}: it names a program on this computer that does not exist in the sandbox"));
        }
    }
    if root.contains_key("hooks") {
        notes.push("hooks are passed into the sandbox as they are; commands in them must exist there".to_string());
    }
    let mut helper_ttl = 0;
    let mut api_host = DEFAULT_API_HOST.to_string();
    if let Some(env) = root.get_mut("env").and_then(Value::as_object_mut) {
        if let Some(url) = env.get("ANTHROPIC_BASE_URL").and_then(Value::as_str).filter(|url| !url.is_empty()) {
            let mut endpoint = parse_url(url)?;
            if ["localhost", "127.0.0.1"].contains(&endpoint.host.as_str()) {
                endpoint.host = HOST_FROM_GUEST.into();
                notes.push(format!("the gateway runs on this computer; the sandbox reaches it as {HOST_FROM_GUEST}:{}", endpoint.port));
            }
            let standard = matches!((endpoint.scheme.as_str(), endpoint.port), ("http", 80) | ("https", 443));
            let port = if standard { String::new() } else { format!(":{}", endpoint.port) };
            env.insert("ANTHROPIC_BASE_URL".into(), format!("{}://{}{port}{}", endpoint.scheme, endpoint.host, endpoint.path).into());
            rules.push(format!("allow {}:{}", endpoint.host, endpoint.port));
            if endpoint.scheme == "http" && endpoint.host.parse::<std::net::Ipv4Addr>().is_err() {
                notes.push(format!(
                    "{} is reached without TLS: its key is only put into requests if the name resolves to a private address",
                    endpoint.host
                ));
            }
            api_host = endpoint.host;
        } else {
            rules.push(format!("allow {DEFAULT_API_HOST}"));
        }
        for (key, name) in CREDENTIALS {
            let Some(value) = env.get(key).and_then(Value::as_str).filter(|v| !v.is_empty()).map(str::to_string) else { continue };
            let placeholder = format!("sk-boxquell-placeholder-{name}");
            env.insert(key.into(), placeholder.clone().into());
            secrets.push(SecretDef { name: name.into(), hosts: vec![api_host.clone()], env: vec![], placeholder, value, helper: None, ttl_secs: 0 });
            notes.push(format!("{key} stays on this computer as the secret {name:?}, sent only to {api_host}"));
        }
        // Claude Code's own setting for how long a helper's key is good, in milliseconds.
        helper_ttl = env
            .get("CLAUDE_CODE_API_KEY_HELPER_TTL_MS")
            .and_then(|ttl| ttl.as_str().and_then(|text| text.parse::<u64>().ok()).or(ttl.as_u64()))
            .unwrap_or(0)
            / 1000;
        if env.contains_key("ANTHROPIC_CUSTOM_HEADERS") {
            notes.push("ANTHROPIC_CUSTOM_HEADERS is passed into the sandbox as it is; remove it if it carries a credential".to_string());
        }
    }
    if rules.is_empty() {
        rules.push(format!("allow {DEFAULT_API_HOST}"));
    }
    if let Some(helper) = key_helper {
        // The helper keeps running on this computer, where its sign-in lives.
        // The sandbox gets a helper that prints a placeholder, so Claude Code
        // sends it the same way; the real key replaces it on the way out.
        let name = "claude-key-helper";
        let placeholder = format!("sk-boxquell-placeholder-{name}");
        settings["apiKeyHelper"] = format!("echo {placeholder}").into();
        notes.push(format!("apiKeyHelper keeps running on this computer; its key is the secret {name:?}, sent only to {api_host}"));
        secrets.push(SecretDef {
            name: name.into(),
            hosts: vec![api_host],
            env: vec![],
            placeholder,
            value: String::new(),
            helper: Some(helper),
            ttl_secs: helper_ttl,
        });
    }
    Ok(Imported { settings: serde_json::to_string_pretty(&settings)?, secrets, rules, notes })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_become_secrets_and_the_gateway_is_allowed() {
        let text =
            r#"{"env":{"ANTHROPIC_BASE_URL":"http://localhost:4000/v1","ANTHROPIC_AUTH_TOKEN":"sk-real","ANTHROPIC_MODEL":"m"},"apiKeyHelper":"/x"}"#;
        let imported = import(text).unwrap();
        assert!(!imported.settings.contains("sk-real") && !imported.settings.contains("/x"));
        // The helper stays on this computer; the sandbox's own prints a placeholder.
        let helper = imported.secrets.iter().find(|secret| secret.helper.is_some()).unwrap();
        assert_eq!((helper.helper.as_deref(), helper.hosts.as_slice()), (Some("/x"), &["10.0.2.2".to_string()][..]));
        assert!(imported.settings.contains(&format!("echo {}", helper.placeholder)));
        assert!(imported.settings.contains("http://10.0.2.2:4000/v1") && imported.settings.contains("\"m\""));
        assert_eq!(imported.rules, ["allow 10.0.2.2:4000"]);
        let secret = &imported.secrets[0];
        assert_eq!(
            (secret.name.as_str(), secret.value.as_str(), secret.hosts.as_slice()),
            ("claude-auth-token", "sk-real", &["10.0.2.2".to_string()][..])
        );
        assert!(imported.settings.contains(&secret.placeholder));
    }

    #[test]
    fn settings_without_a_gateway_use_the_anthropic_api() {
        let imported = import(r#"{"env":{"ANTHROPIC_API_KEY":"k"},"model":"x"}"#).unwrap();
        assert_eq!(imported.rules, ["allow api.anthropic.com"]);
        assert_eq!(imported.secrets[0].hosts, ["api.anthropic.com"]);
        assert!(import("[]").is_err());
        assert!(import(r#"{"env":{"ANTHROPIC_BASE_URL":"ftp://x"}}"#).is_err());
    }
}

#[cfg(test)]
mod merge_tests {
    use super::*;

    #[test]
    fn drop_ins_win_for_values_and_join_lists() {
        let mut base: Value = serde_json::from_str(r#"{"model":"a","env":{"X":"1","Y":"2"},"list":[1,2]}"#).unwrap();
        merge(&mut base, serde_json::from_str(r#"{"model":"b","env":{"Y":"3"},"list":[2,3]}"#).unwrap());
        assert_eq!(base, serde_json::from_str::<Value>(r#"{"model":"b","env":{"X":"1","Y":"3"},"list":[1,2,3]}"#).unwrap());
    }
}
