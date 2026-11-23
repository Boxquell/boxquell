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

/// A new rule goes first so it wins over older ones for the same destination.
async fn add_rule(State(daemon): App, Path(name): Path<String>, Json(body): Json<RuleBody>) -> ApiResult {
    let rule = body.rule;
    Ok(Json(daemon.update_rules(&name, |rules| {
        rules.retain(|r| !(r.host == rule.host && r.port == rule.port));
        rules.insert(0, rule);
    })?))
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
}

async fn recipes(State(daemon): App) -> ApiResult {
    let catalog = boxquell_core::recipe::Catalog::of(&daemon.home)?;
    Ok(Json(serde_json::to_value(catalog.all())?))
}

async fn install_recipes(State(daemon): App, Path(name): Path<String>, Json(body): Json<RecipesBody>) -> ApiResult {
    Ok(Json(daemon.start_install(&name, body.recipes)?))
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

async fn ui(uri: axum::http::Uri) -> Response {
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
    settings: String,
}

async fn set_claude_settings(State(daemon): App, Path(name): Path<String>, Json(body): Json<SettingsBody>) -> ApiResult {
    Ok(Json(daemon.set_claude_settings(&name, &body.settings)?))
}

async fn clear_claude_settings(State(daemon): App, Path(name): Path<String>) -> ApiResult {
    Ok(Json(daemon.clear_claude_settings(&name)?))
}

#[derive(Deserialize)]
struct ResizeBody {
    cpus: Option<u8>,
    ram_mib: Option<u32>,
    disk_gib: Option<u32>,
}

async fn resize(State(daemon): App, Path(name): Path<String>, Json(body): Json<ResizeBody>) -> ApiResult {
    Ok(Json(daemon.resize(&name, body.cpus, body.ram_mib, body.disk_gib)?))
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

async fn ping() -> ApiResult {
    Ok(Json(json!({ "version": env!("CARGO_PKG_VERSION") })))
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
        .route("/api/sandboxes/{name}/ports", post(add_port))
        .route("/api/sandboxes/{name}/ports/{host}", delete(remove_port))
        .route("/api/rules", get(global_rules).post(add_global_rule).delete(remove_global_rule))
        .route("/api/sandboxes/{name}/start", post(start))
        .route("/api/sandboxes/{name}/stop", post(stop))
        .route("/api/sandboxes/{name}/rules", post(add_rule).delete(remove_rule))
        .route("/api/sandboxes/{name}/pause", post(pause))
        .route("/api/sandboxes/{name}/resume", post(resume))
        .route("/api/sandboxes/{name}/mode", post(set_mode))
        .route("/api/sandboxes/{name}/terminal", get(crate::terminal::open))
        .route("/api/sandboxes/{name}/secrets", post(attach_secret))
        .route("/api/sandboxes/{name}/recipes", post(install_recipes))
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
