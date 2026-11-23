//! HTTP API of the daemon, used by the CLI and the UI. Loopback only, and
//! every request needs the token from the daemon file.

use crate::daemon::Daemon;
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use boxquell_core::{EventFilter, SandboxSpec, SecretDef};
use boxquell_net::Rule;
use futures_util::{Stream, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::convert::Infallible;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;

type App = State<Arc<Daemon>>;

struct ApiError(anyhow::Error);

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("{:#}", self.0) }))).into_response()
    }
}

type ApiResult = Result<Json<Value>, ApiError>;

#[derive(Deserialize)]
struct TokenQuery {
    token: Option<String>,
}

/// Accept the token as a bearer header, or as `?token=` for browser event streams.
async fn authorize(State(daemon): App, Query(query): Query<TokenQuery>, request: Request, next: Next) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string);
    let given = bearer.or(query.token).unwrap_or_default();
    let expected = daemon.token.as_bytes();
    let equal = given.len() == expected.len() && given.bytes().zip(expected).fold(0, |acc, (a, b)| acc | (a ^ b)) == 0;
    if equal {
        next.run(request).await
    } else {
        (StatusCode::UNAUTHORIZED, Json(json!({ "error": "missing or wrong token" }))).into_response()
    }
}

#[derive(Deserialize)]
struct RuleBody {
    rule: Rule,
    /// The rule ends by itself after this many minutes.
    #[serde(default)]
    minutes: Option<u64>,
}

#[derive(Deserialize)]
struct SecretRef {
    secret: String,
}

#[derive(Deserialize)]
struct SecretBody {
    value: String,
    hosts: Option<Vec<String>>,
    env: Option<Vec<String>>,
    placeholder: Option<String>,
}

fn public_secret(secret: &SecretDef) -> Value {
    json!({ "name": secret.name, "hosts": secret.hosts, "env": secret.env, "placeholder": secret.placeholder })
}

async fn list(State(daemon): App) -> ApiResult {
    Ok(Json(daemon.list()?.into()))
}

async fn create(State(daemon): App, Json(spec): Json<SandboxSpec>) -> ApiResult {
    Ok(Json(daemon.create(spec)?))
}

async fn show(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(daemon.info(&name)?))
}

async fn remove(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    daemon.remove(&name).await?;
    Ok(Json(json!({})))
}

async fn start(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(daemon.start(&name).await?))
}

async fn stop(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    daemon.info(&name)?;
    daemon.stop(&name).await?;
    Ok(Json(daemon.info(&name)?))
}

async fn add_rule(State(daemon): App, Path(name): Path<String>, Json(body): Json<RuleBody>) -> ApiResult {
    Ok(Json(daemon.add_rule(&name, body.rule, body.minutes)?))
}

async fn continue_after_limit(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(daemon.continue_after_limit(&name)?))
}

#[derive(Deserialize)]
struct AutoRuleBody {
    source: String,
    /// One destination; left out for all of the source's.
    host: Option<String>,
    enabled: bool,
}

async fn set_auto_rule(State(daemon): App, Path(name): Path<String>, Json(body): Json<AutoRuleBody>) -> ApiResult {
    Ok(Json(daemon.set_auto_rule(&name, &body.source, body.host.as_deref(), body.enabled)?))
}

async fn remove_rule(State(daemon): App, Path(name): Path<String>, Json(body): Json<RuleBody>) -> ApiResult {
    Ok(Json(daemon.update_rules(&name, |rules| rules.retain(|r| *r != body.rule))?))
}

async fn attach_secret(State(daemon): App, Path(name): Path<String>, Json(body): Json<SecretRef>) -> ApiResult {
    Ok(Json(daemon.attach_secret(&name, &body.secret)?))
}

async fn pause(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(daemon.set_paused(&name, true)?))
}

async fn resume(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(daemon.set_paused(&name, false)?))
}

#[derive(Deserialize)]
struct ModeBody {
    mode: boxquell_net::Mode,
}

async fn set_mode(State(daemon): App, Path(name): Path<String>, Json(body): Json<ModeBody>) -> ApiResult {
    Ok(Json(daemon.set_mode(&name, body.mode)?))
}

#[derive(Deserialize)]
struct RecipesBody {
    recipes: Vec<String>,
    /// Run the install again although the software is there, to update it.
    #[serde(default)]
    again: bool,
}

async fn recipes(State(daemon): App) -> ApiResult {
    // The first listing fetches the official repositories; git talks to the network.
    let home = daemon.home.clone();
    let catalog = tokio::task::spawn_blocking(move || {
        for problem in boxquell_core::repo::ensure_official(&home) {
            eprintln!("boxquell: {problem}");
        }
        boxquell_core::recipe::Catalog::of(&home)
    })
    .await??;
    Ok(Json(serde_json::to_value(catalog.all())?))
}

async fn install_recipes(State(daemon): App, Path(name): Path<String>, Json(body): Json<RecipesBody>) -> ApiResult {
    Ok(Json(daemon.start_install(&name, body.recipes, "add", body.again)?))
}

async fn remove_recipes(State(daemon): App, Path(name): Path<String>, Json(body): Json<RecipesBody>) -> ApiResult {
    Ok(Json(daemon.start_install(&name, body.recipes, "remove", false)?))
}

async fn forget_recipes(State(daemon): App, Path(name): Path<String>, Json(body): Json<RecipesBody>) -> ApiResult {
    Ok(Json(daemon.forget_recipes(&name, &body.recipes)?))
}

async fn record_recipes(State(daemon): App, Path(name): Path<String>, Json(body): Json<RecipesBody>) -> ApiResult {
    Ok(Json(daemon.record_recipes(&name, &body.recipes)?))
}

#[derive(Deserialize)]
struct RepoBody {
    url: String,
    name: Option<String>,
}

async fn list_repos(State(daemon): App) -> ApiResult {
    Ok(Json(serde_json::to_value(boxquell_core::repo::list(&daemon.home))?))
}

// git talks to the network, so these run off the async threads.
async fn add_repo(State(daemon): App, Json(body): Json<RepoBody>) -> ApiResult {
    let repo = tokio::task::spawn_blocking(move || boxquell_core::repo::add(&daemon.home, &body.url, body.name.as_deref())).await??;
    Ok(Json(serde_json::to_value(repo)?))
}

async fn update_repo(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    let repo = tokio::task::spawn_blocking(move || boxquell_core::repo::update(&daemon.home, &name)).await??;
    Ok(Json(serde_json::to_value(repo)?))
}

async fn remove_repo(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    boxquell_core::repo::remove(&daemon.home, &name)?;
    Ok(Json(json!({})))
}

/// The web UI, built from `ui/` and embedded in the binary. It holds no data;
/// everything it shows comes from the token-protected API.
#[derive(rust_embed::RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../ui/dist"]
struct Ui;

/// How the desktop app names itself in its requests.
pub const DESKTOP_AGENT: &str = "Boxquell-Desktop";

const BROWSER_UI_OFF: &str = "<!doctype html><meta charset=utf-8><meta name=color-scheme content='light dark'><title>Boxquell</title>\
<body style='font:15px/1.5 system-ui,sans-serif;max-width:34em;margin:18vh auto;padding:0 1.5em'>\
<h1 style='font-size:1.4em'>The browser interface is switched off</h1>\
<p>Open the Boxquell app instead. To use Boxquell in a web browser as well, turn it on:</p>\
<p><code>boxquell config browser-ui on</code></p>";

async fn ui(State(daemon): App, headers: axum::http::HeaderMap, uri: axum::http::Uri) -> Response {
    let from_app = headers.get(header::USER_AGENT).and_then(|value| value.to_str().ok()).is_some_and(|agent| agent.contains(DESKTOP_AGENT));
    if !uri.path().starts_with("/api/") && !from_app && !daemon.settings().browser_ui {
        return (StatusCode::FORBIDDEN, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], BROWSER_UI_OFF).into_response();
    }
    // An API path this version does not know must not be answered with the page.
    if uri.path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, Json(json!({ "error": "this daemon does not know that request; it may be older than the page" })))
            .into_response();
    }
    let path = uri.path().trim_start_matches('/');
    let (path, file) = match Ui::get(path) {
        Some(file) => (path, Some(file)),
        None => ("index.html", Ui::get("index.html")),
    };
    match file {
        Some(file) => {
            let mime = match path.rsplit('.').next() {
                Some("html") => "text/html; charset=utf-8",
                Some("js") => "text/javascript",
                Some("css") => "text/css",
                Some("svg") => "image/svg+xml",
                _ => "application/octet-stream",
            };
            ([(header::CONTENT_TYPE, mime), (header::CACHE_CONTROL, "no-cache")], file.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "the web UI is not part of this build; run `npm run build` in ui/ and rebuild boxquell").into_response(),
    }
}

async fn list_secrets(State(daemon): App) -> ApiResult {
    Ok(Json(daemon.secrets.list()?.iter().map(public_secret).collect::<Vec<_>>().into()))
}

/// The stored secret (or the preset of that name) with the request's fields applied.
fn secret_from(existing: Option<SecretDef>, name: &str, body: SecretBody) -> SecretDef {
    let mut secret = existing.or_else(|| SecretDef::preset(name)).unwrap_or_else(|| SecretDef {
        name: name.to_string(),
        hosts: vec![],
        env: vec![],
        placeholder: format!("boxquell-placeholder-{name}"),
        value: String::new(),
        helper: None,
        ttl_secs: 0,
    });
    secret.value = body.value.trim().to_string();
    if let Some(hosts) = body.hosts {
        secret.hosts = hosts.iter().map(|h| h.trim().to_ascii_lowercase()).collect();
    }
    if let Some(env) = body.env {
        secret.env = env;
    }
    if let Some(placeholder) = body.placeholder {
        secret.placeholder = placeholder;
    }
    secret
}

async fn set_local_secret(State(daemon): App, Path((name, secret)): Path<(String, String)>, Json(body): Json<SecretBody>) -> ApiResult {
    boxquell_core::spec::validate_name(&secret)?;
    let def = secret_from(daemon.local_secrets(&name).get(&secret)?, &secret, body);
    if def.hosts.is_empty() {
        return Err(anyhow::anyhow!("name the hosts that may receive {secret:?}").into());
    }
    Ok(Json(daemon.set_local_secret(&name, def)?))
}

async fn remove_local_secret(State(daemon): App, Path((name, secret)): Path<(String, String)>) -> ApiResult {
    Ok(Json(daemon.remove_local_secret(&name, &secret)?))
}

#[derive(Deserialize)]
struct SettingsBody {
    /// The settings as text; left out with `host`.
    #[serde(default)]
    settings: String,
    /// Take this computer's managed settings, drop-in files included. They
    /// are read here and never travel through the page.
    #[serde(default)]
    host: bool,
}

async fn set_claude_settings(State(daemon): App, Path(name): Path<String>, Json(body): Json<SettingsBody>) -> ApiResult {
    // A key helper in the settings is run right away, which can take a moment.
    let info = tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let text = if body.host { boxquell_core::claude::host_managed_text()? } else { body.settings };
        daemon.set_claude_settings(&name, &text)
    })
    .await??;
    Ok(Json(info))
}

#[derive(Deserialize)]
struct HandledBody {
    items: Vec<HandledItem>,
}

#[derive(Deserialize)]
struct HandledItem {
    key: String,
    until: u64,
}

#[derive(Deserialize)]
struct SnapshotBody {
    #[serde(default)]
    label: String,
}

async fn take_snapshot(State(daemon): App, Path(name): Path<String>, Json(body): Json<SnapshotBody>) -> ApiResult {
    daemon.take_snapshot(&name, &body.label).await?;
    Ok(Json(daemon.info(&name)?))
}

async fn restore_snapshot(State(daemon): App, Path((name, id)): Path<(String, String)>) -> ApiResult {
    Ok(Json(daemon.restore_snapshot(&name, &id).await?))
}

async fn delete_snapshot(State(daemon): App, Path((name, id)): Path<(String, String)>) -> ApiResult {
    Ok(Json(daemon.delete_snapshot(&name, &id)?))
}

#[derive(Deserialize)]
struct TemplateBody {
    name: String,
    #[serde(default)]
    description: String,
}

async fn templates(State(daemon): App) -> ApiResult {
    Ok(Json(daemon.templates().into()))
}

async fn save_template(State(daemon): App, Path(name): Path<String>, Json(body): Json<TemplateBody>) -> ApiResult {
    Ok(Json(daemon.save_template(&name, &body.name, &body.description).await?))
}

async fn delete_template(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    daemon.delete_template(&name)?;
    Ok(Json(daemon.templates().into()))
}

#[derive(Deserialize)]
struct ProjectQuery {
    dir: std::path::PathBuf,
}

/// The project file of a folder, if it has one, read for the form that creates a sandbox.
async fn project_file(Query(query): Query<ProjectQuery>) -> ApiResult {
    let file = query.dir.join(boxquell_core::project::FILE);
    if !file.is_file() {
        return Ok(Json(Value::Null));
    }
    let project = boxquell_core::project::load(&file)?;
    Ok(Json(json!({ "file": file, "name": project.sandbox_name(&query.dir), "project": project })))
}

#[derive(Deserialize)]
struct ProjectBody {
    /// Apply the project file of this folder to the sandbox.
    dir: Option<std::path::PathBuf>,
    /// Write the sandbox's description into its project folder, replacing a file that is there.
    #[serde(default)]
    force: bool,
}

async fn apply_project(State(daemon): App, Path(name): Path<String>, Json(body): Json<ProjectBody>) -> ApiResult {
    match body.dir {
        Some(dir) => {
            let project = boxquell_core::project::load(&dir.join(boxquell_core::project::FILE))?;
            Ok(Json(daemon.apply_project(&name, &project).await?))
        }
        None => Ok(Json(json!({ "file": daemon.write_project_file(&name, body.force)? }))),
    }
}

#[derive(Deserialize)]
struct UsageQuery {
    #[serde(default)]
    days: Option<u64>,
}

async fn usage(State(daemon): App, Query(query): Query<UsageQuery>) -> ApiResult {
    Ok(Json(daemon.usage_report(query.days.unwrap_or(7).clamp(1, 90))?))
}

async fn prices(State(daemon): App) -> ApiResult {
    Ok(Json(serde_json::to_value(daemon.prices())?))
}

async fn set_prices(State(daemon): App, Json(body): Json<Vec<crate::usage::Price>>) -> ApiResult {
    daemon.set_prices(&body)?;
    Ok(Json(serde_json::to_value(daemon.prices())?))
}

async fn key_limits(State(daemon): App) -> ApiResult {
    Ok(Json(serde_json::to_value(daemon.key_limits())?))
}

async fn set_key_limits(State(daemon): App, Json(body): Json<std::collections::BTreeMap<String, crate::usage::Limit>>) -> ApiResult {
    daemon.set_key_limits(&body)?;
    Ok(Json(serde_json::to_value(daemon.key_limits())?))
}

#[derive(Deserialize)]
struct TaskBody {
    agent: String,
    prompt: String,
    /// Take a snapshot of the sandbox first.
    #[serde(default)]
    snapshot: bool,
}

async fn start_task(State(daemon): App, Path(name): Path<String>, Json(body): Json<TaskBody>) -> ApiResult {
    Ok(Json(daemon.start_task(&name, &body.agent, &body.prompt, body.snapshot).await?))
}

async fn task_log(State(daemon): App, Path((name, id)): Path<(String, String)>) -> ApiResult {
    Ok(Json(json!({ "text": daemon.task_log(&name, &id)? })))
}

async fn stop_task(State(daemon): App, Path((name, id)): Path<(String, String)>) -> ApiResult {
    daemon.stop_task(&name, &id)?;
    Ok(Json(json!({})))
}

async fn delete_task(State(daemon): App, Path((name, id)): Path<(String, String)>) -> ApiResult {
    daemon.delete_task(&name, &id)?;
    Ok(Json(daemon.info(&name)?))
}

#[derive(Deserialize)]
struct ReviewMode {
    on: bool,
}

async fn set_review(State(daemon): App, Path(name): Path<String>, Json(body): Json<ReviewMode>) -> ApiResult {
    Ok(Json(daemon.set_review(&name, body.on).await?))
}

async fn review_changes(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(tokio::task::spawn_blocking(move || daemon.review_changes(&name)).await??))
}

#[derive(Deserialize)]
struct DiffQuery {
    path: String,
    /// Show the sandbox's state as the old one and the project's as the new one.
    #[serde(default)]
    towards_project: bool,
}

async fn review_diff(State(daemon): App, Path(name): Path<String>, Query(query): Query<DiffQuery>) -> ApiResult {
    Ok(Json(tokio::task::spawn_blocking(move || daemon.review_diff(&name, &query.path, query.towards_project)).await??))
}

#[derive(Deserialize)]
struct SettleBody {
    /// The files; left out for all that fit the action.
    paths: Option<Vec<String>>,
    /// `apply`: the sandbox's state into the project. `discard`: the project's
    /// state back into the sandbox's copy. `bring`: what changed in the project into the copy.
    action: String,
}

async fn review_settle(State(daemon): App, Path(name): Path<String>, Json(body): Json<SettleBody>) -> ApiResult {
    let (to_project, from_project_only) = match body.action.as_str() {
        "apply" => (true, false),
        "discard" => (false, false),
        "bring" => (false, true),
        other => return Err(anyhow::anyhow!("unknown action {other:?}").into()),
    };
    Ok(Json(tokio::task::spawn_blocking(move || daemon.review_settle(&name, body.paths, to_project, from_project_only)).await??))
}

async fn traffic(State(daemon): App, Path(name): Path<String>, Query(query): Query<UsageQuery>) -> ApiResult {
    Ok(Json(daemon.traffic_report(&name, query.days.unwrap_or(7).clamp(1, 90))?))
}

/// What is needed to connect an assistant to `boxquell mcp`: where the program
/// is, and whether Claude Code is on this computer to be set up with a click.
async fn mcp_info() -> ApiResult {
    let exe = crate::daemon::program();
    let claude = which("claude");
    let connected = match &claude {
        Some(claude) => tokio::process::Command::new(claude)
            .args(["mcp", "get", "boxquell"])
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .is_ok_and(|output| output.status.success()),
        None => false,
    };
    Ok(Json(json!({ "exe": exe, "claude": claude.is_some(), "claude_connected": connected })))
}

/// A program on the PATH or in the usual places of a user's own programs.
fn which(name: &str) -> Option<std::path::PathBuf> {
    let home = std::env::home_dir();
    let extra = home.iter().flat_map(|home| [home.join(".local/bin"), home.join(".claude/local"), home.join(".npm-global/bin")]);
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .chain(extra)
        .map(|dir| dir.join(name))
        .find(|file| file.is_file())
}

#[derive(Deserialize)]
struct McpConnect {
    /// `true` adds Boxquell to Claude Code for this user, `false` takes it out again.
    connect: bool,
}

async fn mcp_connect(Json(body): Json<McpConnect>) -> ApiResult {
    let claude = which("claude").ok_or_else(|| anyhow::anyhow!("Claude Code was not found on this computer"))?;
    let exe = crate::daemon::program().ok_or_else(|| anyhow::anyhow!("the path of the boxquell program is unknown"))?;
    let mut command = tokio::process::Command::new(claude);
    if body.connect {
        command.args(["mcp", "add", "--scope", "user", "boxquell", "--"]).arg(exe).arg("mcp");
    } else {
        command.args(["mcp", "remove", "--scope", "user", "boxquell"]);
    }
    let output = command.stdin(std::process::Stdio::null()).output().await?;
    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!("Claude Code said: {}", said.lines().last().unwrap_or("nothing").trim()).into());
    }
    mcp_info().await
}

#[derive(Deserialize)]
struct ActionBody {
    recipe: String,
    /// Which of the recipe's actions, counted from 0.
    action: usize,
    #[serde(default)]
    values: std::collections::BTreeMap<String, String>,
}

async fn run_action(State(daemon): App, Path(name): Path<String>, Json(body): Json<ActionBody>) -> ApiResult {
    Ok(Json(daemon.run_action(&name, &body.recipe, body.action, &body.values).await?))
}

async fn handled_notifications(State(daemon): App) -> ApiResult {
    Ok(Json(serde_json::to_value(daemon.handled_notifications())?))
}

async fn mark_handled(State(daemon): App, Json(body): Json<HandledBody>) -> ApiResult {
    Ok(Json(serde_json::to_value(daemon.mark_handled(body.items.into_iter().map(|item| (item.key, item.until)).collect())?)?))
}

async fn host_stats(State(daemon): App) -> ApiResult {
    Ok(Json(daemon.host_stats()))
}

/// Whether this computer has managed Claude Code settings to take over.
async fn host_claude_settings() -> ApiResult {
    Ok(Json(json!({ "path": boxquell_core::claude::host_managed_settings(), "present": boxquell_core::claude::host_managed_text().is_ok() })))
}

async fn clear_claude_settings(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(daemon.clear_claude_settings(&name)?))
}

#[derive(Deserialize)]
struct ResizeBody {
    cpus: Option<u8>,
    ram_mib: Option<u32>,
    disk_gib: Option<u32>,
    /// `default` or an empty text goes back to /workspace.
    workdir: Option<String>,
    /// Replaces the list of what starts with the sandbox.
    autostart: Option<Vec<String>>,
    /// When to stop the idle sandbox: minutes, 0 for never, -1 for the setting of all sandboxes.
    idle_stop_minutes: Option<i64>,
    /// Speed limits in kbit per second; 0 takes one away.
    upload_kbit: Option<u64>,
    download_kbit: Option<u64>,
    /// Data limits in MiB; 0 takes one away.
    upload_limit_mib: Option<u64>,
    download_limit_mib: Option<u64>,
    /// Variables to set; an empty value removes the variable.
    #[serde(default)]
    env: std::collections::BTreeMap<String, String>,
}

async fn resize(State(daemon): App, Path(name): Path<String>, Json(body): Json<ResizeBody>) -> ApiResult {
    if let Some(workdir) = body.workdir {
        let workdir = Some(workdir.trim().to_string()).filter(|w| !w.is_empty() && w != "default");
        daemon.set_workdir(&name, workdir)?;
    }
    daemon.set_env(&name, body.env)?;
    daemon.set_data_limits(&name, body.upload_limit_mib, body.download_limit_mib)?;
    daemon.set_speed_limits(&name, body.upload_kbit, body.download_kbit)?;
    if let Some(minutes) = body.idle_stop_minutes {
        daemon.set_idle_stop(&name, u64::try_from(minutes).ok())?;
    }
    if let Some(autostart) = body.autostart {
        daemon.set_autostart(&name, autostart)?;
    }
    Ok(Json(daemon.resize(&name, body.cpus, body.ram_mib, body.disk_gib)?))
}

#[derive(Deserialize)]
struct ShareRef {
    guest: String,
}

async fn add_share(State(daemon): App, Path(name): Path<String>, Json(share): Json<boxquell_core::Share>) -> ApiResult {
    Ok(Json(daemon.add_share(&name, share)?))
}

async fn remove_share(State(daemon): App, Path(name): Path<String>, Json(body): Json<ShareRef>) -> ApiResult {
    Ok(Json(daemon.remove_share(&name, &body.guest)?))
}

async fn add_port(State(daemon): App, Path(name): Path<String>, Json(port): Json<boxquell_core::PortForward>) -> ApiResult {
    Ok(Json(daemon.add_port(&name, port).await?))
}

async fn remove_port(State(daemon): App, Path((name, host)): Path<(String, u16)>) -> ApiResult {
    Ok(Json(daemon.remove_port(&name, host)?))
}

async fn global_rules(State(daemon): App) -> ApiResult {
    Ok(Json(serde_json::to_value(daemon.global_rules())?))
}

async fn add_global_rule(State(daemon): App, Json(body): Json<RuleBody>) -> ApiResult {
    let rule = body.rule;
    Ok(Json(serde_json::to_value(daemon.update_global_rules(|rules| {
        rules.retain(|r| !(r.host == rule.host && r.port == rule.port));
        rules.insert(0, rule);
    })?)?))
}

async fn remove_global_rule(State(daemon): App, Json(body): Json<RuleBody>) -> ApiResult {
    Ok(Json(serde_json::to_value(daemon.update_global_rules(|rules| rules.retain(|r| *r != body.rule))?)?))
}

async fn set_secret(State(daemon): App, Path(name): Path<String>, Json(body): Json<SecretBody>) -> ApiResult {
    let secret = secret_from(daemon.secrets.get(&name)?, &name, body);
    let public = public_secret(&secret);
    daemon.secrets.set(secret)?;
    daemon.refresh_secrets()?;
    Ok(Json(public))
}

async fn remove_secret(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    let existed = daemon.secrets.remove(&name)?;
    daemon.refresh_secrets()?;
    Ok(Json(json!({ "removed": existed })))
}

async fn events(State(daemon): App, Query(filter): Query<EventFilter>) -> ApiResult {
    Ok(Json(daemon.store.query(&filter)?.into()))
}

/// Server-sent events: every new log entry, optionally for one sandbox or blocked attempts only.
async fn event_stream(State(daemon): App, Query(filter): Query<EventFilter>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = BroadcastStream::new(daemon.live.subscribe()).filter_map(move |item| {
        let event = item.ok().filter(|event| {
            let sandbox_ok = filter.sandbox.as_deref().is_none_or(|s| event["sandbox"] == s);
            sandbox_ok && !(filter.blocked_only && event["allowed"] == true)
        });
        std::future::ready(event.map(|event| Ok(Event::default().data(event.to_string()))))
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}

#[derive(Deserialize)]
struct LockdownBody {
    on: bool,
}

async fn lockdown(State(daemon): App) -> ApiResult {
    Ok(Json(json!({ "on": daemon.lockdown() })))
}

async fn set_lockdown(State(daemon): App, Json(body): Json<LockdownBody>) -> ApiResult {
    daemon.set_lockdown(body.on)?;
    Ok(Json(json!({ "on": daemon.lockdown() })))
}

/// IPv4 addresses of this computer that the UI could be opened on.
async fn addresses() -> ApiResult {
    // An address can be listed once per interface entry; show each address once.
    let mut seen = std::collections::BTreeSet::new();
    let list: Vec<Value> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter(|interface| !interface.is_loopback() && interface.ip().is_ipv4() && seen.insert(interface.ip()))
        .map(|interface| json!({ "address": interface.ip(), "interface": interface.name }))
        .collect();
    Ok(Json(list.into()))
}

async fn ssh_keys(State(daemon): App) -> ApiResult {
    Ok(Json(daemon.ssh_keys(None)?))
}

async fn set_ssh_keys(State(daemon): App, Json(body): Json<boxquell_core::spec::SshKeys>) -> ApiResult {
    Ok(Json(daemon.set_ssh_keys(None, body)?))
}

async fn sandbox_ssh_keys(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(daemon.ssh_keys(Some(&name))?))
}

async fn set_sandbox_ssh_keys(State(daemon): App, Path(name): Path<String>, Json(body): Json<boxquell_core::spec::SshKeys>) -> ApiResult {
    Ok(Json(daemon.set_ssh_keys(Some(&name), body)?))
}

async fn settings(State(daemon): App) -> ApiResult {
    Ok(Json(serde_json::to_value(daemon.settings())?))
}

async fn set_settings(State(daemon): App, Json(body): Json<crate::daemon::Settings>) -> ApiResult {
    daemon.set_settings(&body)?;
    Ok(Json(serde_json::to_value(daemon.settings())?))
}

#[derive(Deserialize)]
struct AgentConfigBody {
    instructions: String,
    mcp: String,
}

#[derive(Deserialize)]
struct SkillBody {
    /// Directory on this computer that holds the skill (with its SKILL.md).
    path: std::path::PathBuf,
}

/// The agent configuration for all sandboxes (`None`) or for one.
fn agent_dir(daemon: &Daemon, sandbox: Option<&str>) -> Result<std::path::PathBuf, ApiError> {
    if let Some(name) = sandbox {
        daemon.info(name)?;
    }
    Ok(boxquell_core::agentcfg::dir(&daemon.home, sandbox))
}

fn agent_config(dir: &std::path::Path) -> ApiResult {
    Ok(Json(serde_json::to_value(boxquell_core::agentcfg::load(dir))?))
}

#[derive(Deserialize)]
struct ImportBody {
    #[serde(default)]
    instructions: Vec<String>,
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    mcp: Vec<String>,
}

#[derive(Deserialize)]
struct ScopeQuery {
    sandbox: Option<String>,
}

/// Skills, instructions and MCP servers of this computer's own agents that the set does not have yet.
async fn agent_suggestions(State(daemon): App, Query(scope): Query<ScopeQuery>) -> ApiResult {
    let dir = agent_dir(&daemon, scope.sandbox.as_deref())?;
    Ok(Json(serde_json::to_value(boxquell_core::agentcfg::suggestions(&dir))?))
}

async fn agent_import(State(daemon): App, Query(scope): Query<ScopeQuery>, Json(body): Json<ImportBody>) -> ApiResult {
    let dir = agent_dir(&daemon, scope.sandbox.as_deref())?;
    boxquell_core::agentcfg::import(&dir, &body.instructions, &body.skills, &body.mcp)?;
    agent_config(&dir)
}

async fn global_agent_config(State(daemon): App) -> ApiResult {
    agent_config(&agent_dir(&daemon, None)?)
}

async fn set_global_agent_config(State(daemon): App, Json(body): Json<AgentConfigBody>) -> ApiResult {
    let dir = agent_dir(&daemon, None)?;
    boxquell_core::agentcfg::save(&dir, &body.instructions, &body.mcp)?;
    agent_config(&dir)
}

async fn add_global_skill(State(daemon): App, Json(body): Json<SkillBody>) -> ApiResult {
    let dir = agent_dir(&daemon, None)?;
    boxquell_core::agentcfg::add_skill(&dir, &body.path)?;
    agent_config(&dir)
}

async fn remove_global_skill(State(daemon): App, Path(skill): Path<String>) -> ApiResult {
    let dir = agent_dir(&daemon, None)?;
    boxquell_core::agentcfg::remove_skill(&dir, &skill)?;
    agent_config(&dir)
}

async fn sandbox_agent_config(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    agent_config(&agent_dir(&daemon, Some(&name))?)
}

async fn set_sandbox_agent_config(State(daemon): App, Path(name): Path<String>, Json(body): Json<AgentConfigBody>) -> ApiResult {
    let dir = agent_dir(&daemon, Some(&name))?;
    boxquell_core::agentcfg::save(&dir, &body.instructions, &body.mcp)?;
    agent_config(&dir)
}

async fn add_sandbox_skill(State(daemon): App, Path(name): Path<String>, Json(body): Json<SkillBody>) -> ApiResult {
    let dir = agent_dir(&daemon, Some(&name))?;
    boxquell_core::agentcfg::add_skill(&dir, &body.path)?;
    agent_config(&dir)
}

async fn remove_sandbox_skill(State(daemon): App, Path((name, skill)): Path<(String, String)>) -> ApiResult {
    let dir = agent_dir(&daemon, Some(&name))?;
    boxquell_core::agentcfg::remove_skill(&dir, &skill)?;
    agent_config(&dir)
}

#[derive(Deserialize)]
struct FolderQuery {
    /// The folder to list; the home folder when left out.
    path: Option<std::path::PathBuf>,
    #[serde(default)]
    hidden: bool,
}

/// The folders inside a folder of this computer, for choosing one in the interface.
async fn folders(Query(query): Query<FolderQuery>) -> ApiResult {
    let start =
        query.path.filter(|path| path.is_absolute()).or_else(std::env::home_dir).ok_or_else(|| anyhow::anyhow!("cannot find your home folder"))?;
    let path = std::fs::canonicalize(&start).map_err(|err| anyhow::anyhow!("{}: {err}", start.display()))?;
    let mut names: Vec<String> = std::fs::read_dir(&path)
        .map_err(|err| anyhow::anyhow!("{}: {err}", path.display()))?
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| query.hidden || !name.starts_with('.'))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    Ok(Json(json!({ "path": path, "parent": path.parent(), "folders": names, "home": std::env::home_dir() })))
}

#[derive(Deserialize)]
struct TailQuery {
    /// How many lines from the end; 400 when left out.
    lines: Option<usize>,
}

/// The last lines of a text file; empty when it does not exist.
fn tail(path: &std::path::Path, lines: Option<usize>) -> String {
    let text = std::fs::read(path).map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines.unwrap_or(400).min(5000))..].join("\n")
}

/// What the daemon itself wrote: start-up, errors, leftover processes it ended.
async fn daemon_log(State(daemon): App, Query(query): Query<TailQuery>) -> ApiResult {
    Ok(Json(json!({ "text": tail(&daemon.home.root().join("daemon.log"), query.lines) })))
}

/// What a sandbox printed while booting and running: kernel and start script messages.
async fn console_log(State(daemon): App, Path(name): Path<String>, Query(query): Query<TailQuery>) -> ApiResult {
    daemon.info(&name)?;
    Ok(Json(json!({ "text": tail(&daemon.home.sandbox(&name).join("console.log"), query.lines) })))
}

#[derive(Deserialize)]
struct OpenBody {
    url: String,
}

/// Open a web address in the user's browser. The desktop app's window cannot
/// open links itself, so it asks the daemon, which runs as the same user.
async fn open_link(Json(body): Json<OpenBody>) -> ApiResult {
    let url = body.url.trim();
    let web = (url.starts_with("https://") || url.starts_with("http://")) && !url.contains(char::is_whitespace) && !url.contains(char::is_control);
    if !web {
        return Err(anyhow::anyhow!("only web addresses can be opened").into());
    }
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    Ok(Json(json!({})))
}

/// What this computer has, so the interface can offer sensible limits for a sandbox.
async fn host() -> ApiResult {
    let cpus = std::thread::available_parallelism().map_or(4, |n| n.get());
    // Linux tells its memory in /proc; elsewhere the interface falls back to a fixed range.
    let ram_mib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| text.lines().find_map(|line| line.strip_prefix("MemTotal:")?.split_whitespace().next()?.parse::<u64>().ok()))
        .map(|kib| kib / 1024);
    Ok(Json(json!({ "cpus": cpus, "ram_mib": ram_mib, "default_backend": boxquell_core::Backend::default_for_host() })))
}

/// Version, changelog and where the project lives.
async fn about() -> ApiResult {
    Ok(Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "changelog": include_str!("../../../CHANGELOG.md"),
        "repository": REPOSITORY,
        "license": "MIT OR Apache-2.0",
    })))
}

/// The licenses of everything Boxquell is built from.
async fn notices() -> ApiResult {
    Ok(Json(json!({ "text": include_str!("../../../THIRD-PARTY-NOTICES.md") })))
}

const REPOSITORY: &str = "https://github.com/Boxquell/boxquell";

/// Compare two versions such as `0.1.0` and `v0.2.0-rc1` by their numbers.
fn newer(latest: &str, current: &str) -> bool {
    let numbers =
        |version: &str| -> Vec<u64> { version.trim_start_matches('v').split(['.', '-', '+']).map_while(|part| part.parse().ok()).collect() };
    numbers(latest) > numbers(current)
}

/// Ask GitHub for the latest release; nothing but the request itself is sent.
pub fn latest_release() -> anyhow::Result<Value> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(10))).http_status_as_error(false).build().into();
    let mut response = agent
        .get("https://api.github.com/repos/Boxquell/boxquell/releases/latest")
        .header("User-Agent", "boxquell")
        .header("Accept", "application/vnd.github+json")
        .call()?;
    let status = response.status();
    let body: Value = response.body_mut().read_json().unwrap_or_default();
    let current = env!("CARGO_PKG_VERSION");
    if status.as_u16() == 404 {
        return Ok(json!({ "current": current, "latest": null, "newer": false, "note": "No release has been published yet." }));
    }
    anyhow::ensure!(status.is_success(), "GitHub answered {status}");
    let latest = body["tag_name"].as_str().unwrap_or_default().to_string();
    Ok(json!({ "current": current, "latest": latest, "newer": newer(&latest, current), "url": body["html_url"], "notes": body["body"] }))
}

/// Look for a newer release now, because the user asked.
async fn update_check(State(daemon): App) -> ApiResult {
    let answer = tokio::task::spawn_blocking(latest_release).await??;
    *daemon.release.lock().unwrap() = Some(answer.clone());
    Ok(Json(answer))
}

/// What the last look for a newer release found; `null` before the first one.
async fn update_status(State(daemon): App) -> ApiResult {
    Ok(Json(daemon.release.lock().unwrap().clone().unwrap_or(Value::Null)))
}

/// The operating systems on offer, and which of them are built.
async fn systems(State(daemon): App) -> ApiResult {
    let home = daemon.home.clone();
    let list = tokio::task::spawn_blocking(move || {
        boxquell_core::repo::ensure_official(&home);
        boxquell_core::os::definitions(&home)
    })
    .await?;
    // The setup scripts are long and of no use to a listing.
    let list: Vec<Value> = list
        .iter()
        .map(|def| json!({ "id": def.id, "name": def.name, "description": def.description, "icon": def.icon, "icon_color": def.icon_color, "group": def.group, "version": def.version, "family": def.family, "source": def.source, "built": def.built, "job": daemon.os_job(&def.id) }))
        .collect();
    Ok(Json(list.into()))
}

/// Build a system's image in the background; progress shows up in the list of systems.
async fn build_system(State(daemon): App, Path(id): Path<String>) -> ApiResult {
    daemon.start_os_build(&id)?;
    Ok(Json(json!({})))
}

async fn stats(State(daemon): App) -> ApiResult {
    Ok(Json(daemon.stats()))
}

/// Raised whenever the web UI starts to need something new from the API, so a
/// page served next to an older daemon can say so instead of failing quietly.
/// Keep in step with `API_LEVEL` in `ui/src/api.ts`.
const API_LEVEL: u32 = 2;

async fn ping() -> ApiResult {
    Ok(Json(json!({ "version": env!("CARGO_PKG_VERSION"), "api": API_LEVEL })))
}

async fn shutdown(State(daemon): App) -> ApiResult {
    daemon.shutdown.notify_one();
    Ok(Json(json!({})))
}

pub fn router(daemon: Arc<Daemon>) -> Router {
    Router::new()
        .route("/api/ping", get(ping))
        .route("/api/shutdown", post(shutdown))
        .route("/api/sandboxes", get(list).post(create))
        .route("/api/sandboxes/{name}", get(show).delete(remove).patch(resize))
        .route("/api/sandboxes/{name}/secrets/{secret}", put(set_local_secret).delete(remove_local_secret))
        .route("/api/sandboxes/{name}/claude-settings", put(set_claude_settings).delete(clear_claude_settings))
        .route("/api/sandboxes/{name}/shares", post(add_share).delete(remove_share))
        .route("/api/sandboxes/{name}/ports", post(add_port))
        .route("/api/sandboxes/{name}/ports/{host}", delete(remove_port))
        .route("/api/stats", get(stats))
        .route("/api/os", get(systems))
        .route("/api/os/{id}/build", post(build_system))
        .route("/api/folders", get(folders))
        .route("/api/open", post(open_link))
        .route("/api/host", get(host))
        .route("/api/about", get(about))
        .route("/api/notices", get(notices))
        .route("/api/claude-settings/host", get(host_claude_settings))
        .route("/api/host-load", get(host_stats))
        .route("/api/notifications/handled", get(handled_notifications).post(mark_handled))
        .route("/api/update-check", get(update_check))
        .route("/api/update-status", get(update_status))
        .route("/api/logs/daemon", get(daemon_log))
        .route("/api/sandboxes/{name}/console", get(console_log))
        .route("/api/agent-config", get(global_agent_config).put(set_global_agent_config))
        .route("/api/agent-suggestions", get(agent_suggestions).post(agent_import))
        .route("/api/agent-config/skills", post(add_global_skill))
        .route("/api/agent-config/skills/{skill}", delete(remove_global_skill))
        .route("/api/sandboxes/{name}/agent-config", get(sandbox_agent_config).put(set_sandbox_agent_config))
        .route("/api/sandboxes/{name}/agent-config/skills", post(add_sandbox_skill))
        .route("/api/sandboxes/{name}/agent-config/skills/{skill}", delete(remove_sandbox_skill))
        .route("/api/lockdown", get(lockdown).post(set_lockdown))
        .route("/api/settings", get(settings).put(set_settings))
        .route("/api/ssh-keys", get(ssh_keys).put(set_ssh_keys))
        .route("/api/sandboxes/{name}/ssh-keys", get(sandbox_ssh_keys).put(set_sandbox_ssh_keys))
        .route("/api/addresses", get(addresses))
        .route("/api/rules", get(global_rules).post(add_global_rule).delete(remove_global_rule))
        .route("/api/sandboxes/{name}/start", post(start))
        .route("/api/sandboxes/{name}/stop", post(stop))
        .route("/api/sandboxes/{name}/rules", post(add_rule).delete(remove_rule))
        .route("/api/sandboxes/{name}/auto-rules", post(set_auto_rule))
        .route("/api/sandboxes/{name}/traffic/continue", post(continue_after_limit))
        .route("/api/sandboxes/{name}/traffic", get(traffic))
        .route("/api/sandboxes/{name}/pause", post(pause))
        .route("/api/sandboxes/{name}/resume", post(resume))
        .route("/api/sandboxes/{name}/mode", post(set_mode))
        .route("/api/sandboxes/{name}/terminal", get(crate::terminal::open))
        .route("/api/sandboxes/{name}/secrets", post(attach_secret))
        .route("/api/sandboxes/{name}/recipes", post(install_recipes).delete(remove_recipes))
        .route("/api/sandboxes/{name}/forgotten", post(forget_recipes))
        .route("/api/sandboxes/{name}/snapshots", post(take_snapshot))
        .route("/api/sandboxes/{name}/snapshots/{id}", post(restore_snapshot).delete(delete_snapshot))
        .route("/api/sandboxes/{name}/template", post(save_template))
        .route("/api/templates", get(templates))
        .route("/api/sandboxes/{name}/actions", post(run_action))
        .route("/api/mcp", get(mcp_info).post(mcp_connect))
        .route("/api/sandboxes/{name}/review", get(review_changes).put(set_review).post(review_settle))
        .route("/api/sandboxes/{name}/review/diff", get(review_diff))
        .route("/api/sandboxes/{name}/tasks", post(start_task))
        .route("/api/sandboxes/{name}/tasks/{id}", get(task_log).delete(delete_task))
        .route("/api/sandboxes/{name}/tasks/{id}/stop", post(stop_task))
        .route("/api/usage", get(usage))
        .route("/api/prices", get(prices).put(set_prices))
        .route("/api/key-limits", get(key_limits).put(set_key_limits))
        .route("/api/project", get(project_file))
        .route("/api/sandboxes/{name}/project", post(apply_project))
        .route("/api/templates/{name}", delete(delete_template))
        .route("/api/sandboxes/{name}/installed", post(record_recipes))
        .route("/api/recipes", get(recipes))
        .route("/api/repos", get(list_repos).post(add_repo))
        .route("/api/repos/{name}", post(update_repo).delete(remove_repo))
        .route("/api/secrets", get(list_secrets))
        .route("/api/secrets/{name}", put(set_secret).delete(remove_secret))
        .route("/api/events", get(events))
        .route("/api/events/stream", get(event_stream))
        .layer(middleware::from_fn_with_state(daemon.clone(), authorize))
        .fallback(ui)
        .with_state(daemon)
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("0.10.0", "0.9.9"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
        assert!(!newer("", "0.1.0"));
    }
}
