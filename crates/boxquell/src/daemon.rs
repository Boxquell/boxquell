//! The per-user background process. It owns every running sandbox: the VM
//! process, its network gateway, the SSH forward, and the event log.

use crate::ssh;
use anyhow::{ensure, Context, Result};
use boxquell_core::drive::{self, shell_quote, DriveFile};
use boxquell_core::spec::validate_name;
use boxquell_core::{Backend, EventStore, Home, PortForward, SandboxSpec, SecretDef, SecretStore};
use boxquell_net::{CertAuthority, GatewayHandle, Intercept, Policy, Rule, Secret, SystemUpstream};
use boxquell_vm::{NetLink, VmSpec};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc, watch, Notify};

const BOOT_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

trait Link: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> Link for T {}

enum LinkListener {
    #[cfg(unix)]
    Unix(tokio::net::UnixListener),
    #[cfg_attr(unix, allow(dead_code))]
    Tcp(TcpListener),
}

impl LinkListener {
    /// Unix socket in the sandbox directory where possible, loopback TCP otherwise.
    async fn bind(dir: &Path) -> Result<(Self, NetLink)> {
        #[cfg(unix)]
        {
            let path = net_socket(dir);
            let _ = std::fs::remove_file(&path);
            let listener = tokio::net::UnixListener::bind(&path).with_context(|| format!("cannot bind {}", path.display()))?;
            Ok((Self::Unix(listener), NetLink::Unix(path)))
        }
        #[cfg(not(unix))]
        {
            let _ = dir;
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let port = listener.local_addr()?.port();
            Ok((Self::Tcp(listener), NetLink::Tcp(port)))
        }
    }

    async fn accept(&self) -> std::io::Result<Box<dyn Link>> {
        match self {
            #[cfg(unix)]
            Self::Unix(listener) => Ok(Box::new(listener.accept().await?.0)),
            Self::Tcp(listener) => Ok(Box::new(listener.accept().await?.0)),
        }
    }
}

struct Running {
    policy: Arc<RwLock<Policy>>,
    secrets: Arc<RwLock<Vec<Secret>>>,
    ssh_port: u16,
    gateway: GatewayHandle,
    /// Recent load measurements from inside the guest, oldest first.
    samples: Mutex<std::collections::VecDeque<Sample>>,
    /// Connection attempts since the sandbox started.
    allowed: std::sync::atomic::AtomicU64,
    blocked: std::sync::atomic::AtomicU64,
    /// Port forwards by host port; ended when the sandbox stops.
    forwards: Mutex<HashMap<u16, tokio::task::JoinHandle<()>>>,
    /// The VM process, for pausing.
    pid: Option<u32>,
    paused: std::sync::atomic::AtomicBool,
    stop: Arc<Notify>,
    exited: watch::Receiver<bool>,
}

/// One load measurement taken inside the guest.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Sample {
    pub ts_ms: u64,
    /// Busy share of all CPUs since the previous sample, 0 to 100.
    pub cpu: f32,
    pub mem_used_mib: u32,
    pub mem_total_mib: u32,
    pub disk_used_gib: f32,
    pub disk_total_gib: f32,
    /// Bytes per second through the guest's network device.
    pub rx_rate: u64,
    pub tx_rate: u64,
}

const SAMPLE_EVERY: u64 = 2;
const SAMPLES_KEPT: usize = 90;

/// Prints one line of counters every few seconds: CPU ticks (total, idle),
/// memory (total, available, kB), root disk (total, used, kB), network bytes (received, sent).
const SAMPLER: &str = r#"while :; do read -r _ u n s i w q sq st _ < /proc/stat; mt=0; ma=0; while read -r k v _; do case $k in MemTotal:) mt=$v;; MemAvailable:) ma=$v;; esac; done < /proc/meminfo; set -- $(df -k / | tail -n1); rx=0; tx=0; for d in /sys/class/net/e*; do [ -r "$d/statistics/rx_bytes" ] && rx=$(cat "$d/statistics/rx_bytes") && tx=$(cat "$d/statistics/tx_bytes"); done; echo "$((u+n+s+i+w+q+sq+st)) $((i+w)) $mt $ma $2 $3 $rx $tx"; sleep 2; done"#;

/// Keep a measuring loop running in the guest over ssh and record what it prints.
/// It is restarted when it ends, e.g. while the guest still boots.
async fn sample_load(running: Arc<Running>, key: PathBuf) {
    use tokio::io::AsyncBufReadExt;
    loop {
        let child = tokio::process::Command::new("ssh")
            .args(ssh::base_args(&key, running.ssh_port))
            .arg(SAMPLER)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn();
        if let Ok(mut child) = child {
            let mut previous: Option<[u64; 8]> = None;
            if let Some(stdout) = child.stdout.take() {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let numbers: Vec<u64> = line.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                    let Ok(now) = <[u64; 8]>::try_from(numbers) else { continue };
                    if let Some(before) = previous {
                        let ticks = now[0].saturating_sub(before[0]).max(1);
                        let idle = now[1].saturating_sub(before[1]).min(ticks);
                        let sample = Sample {
                            ts_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
                            cpu: 100.0 * (ticks - idle) as f32 / ticks as f32,
                            mem_used_mib: (now[2].saturating_sub(now[3]) / 1024) as u32,
                            mem_total_mib: (now[2] / 1024) as u32,
                            disk_used_gib: now[5] as f32 / 1_048_576.0,
                            disk_total_gib: now[4] as f32 / 1_048_576.0,
                            rx_rate: now[6].saturating_sub(before[6]) / SAMPLE_EVERY,
                            tx_rate: now[7].saturating_sub(before[7]) / SAMPLE_EVERY,
                        };
                        let mut samples = running.samples.lock().unwrap();
                        if samples.len() == SAMPLES_KEPT {
                            samples.pop_front();
                        }
                        samples.push_back(sample);
                    }
                    previous = Some(now);
                }
            }
            let _ = child.kill().await;
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

const NOTIFY_QUIET: Duration = Duration::from_secs(60);

/// Preferences stored in `settings.json`.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Desktop notification when a sandbox's connection is blocked.
    pub notify_blocked: bool,
    /// Message in the web UI when a sandbox's connection is blocked.
    pub toast_blocked: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self { notify_blocked: true, toast_blocked: true }
    }
}

/// A recipe install running as `boxquell add` in a child process.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Job {
    /// `running`, `done` or `failed`
    pub state: &'static str,
    pub recipes: Vec<String>,
    pub log: String,
}

pub struct Daemon {
    pub home: Home,
    pub store: EventStore,
    pub secrets: SecretStore,
    pub token: String,
    /// Every stored event, for live views.
    pub live: broadcast::Sender<Value>,
    pub shutdown: Notify,
    /// When a blocked destination was last announced, to avoid a notification per retry.
    notified: Mutex<HashMap<(String, String), std::time::Instant>>,
    ca: Arc<CertAuthority>,
    roots: Arc<boxquell_net::mitm::RootCertStore>,
    running: Mutex<HashMap<String, Arc<Running>>>,
    /// Latest recipe install per sandbox, started from the UI.
    jobs: Mutex<HashMap<String, Job>>,
    /// Serializes start, stop and removal.
    lifecycle: tokio::sync::Mutex<()>,
}

/// Copy a file, leaving all-zero blocks as holes so images stay small on disk.
fn sparse_copy(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut src = std::fs::File::open(from)?;
    let mut dst = std::fs::File::create(to)?;
    let len = src.metadata()?.len();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if buf[..n].iter().all(|b| *b == 0) {
            dst.seek(SeekFrom::Current(n as i64))?;
        } else {
            dst.write_all(&buf[..n])?;
        }
    }
    dst.set_len(len)
}

fn tail(path: &Path, lines: usize) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let all: Vec<&str> = text.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

fn net_secret(def: &SecretDef) -> Secret {
    Secret { name: def.name.clone(), hosts: def.hosts.clone(), placeholder: def.placeholder.clone(), value: def.value.clone() }
}

impl Daemon {
    pub fn new(home: Home) -> Result<Arc<Self>> {
        let mut token = [0u8; 24];
        getrandom::fill(&mut token).map_err(|e| anyhow::anyhow!("no random source: {e}"))?;
        let mut roots = boxquell_net::mitm::RootCertStore::empty();
        roots.add_parsable_certificates(boxquell_net::mitm::native_certs());
        Ok(Arc::new(Self {
            notified: Mutex::new(HashMap::new()),
            store: EventStore::open(&home.events_db())?,
            secrets: SecretStore::new(&home.secrets_file()),
            ca: Arc::new(CertAuthority::load_or_create(home.root())?),
            roots: Arc::new(roots),
            token: token.iter().map(|b| format!("{b:02x}")).collect(),
            live: broadcast::channel(1024).0,
            shutdown: Notify::new(),
            running: Mutex::new(HashMap::new()),
            jobs: Mutex::new(HashMap::new()),
            lifecycle: tokio::sync::Mutex::new(()),
            home,
        }))
    }

    fn running(&self, name: &str) -> Option<Arc<Running>> {
        self.running.lock().unwrap().get(name).cloned()
    }

    fn spec(&self, name: &str) -> Result<SandboxSpec> {
        validate_name(name)?;
        let dir = self.home.sandbox(name);
        ensure!(dir.join("sandbox.toml").exists(), "no sandbox named {name:?}");
        SandboxSpec::load(&dir)
    }

    pub fn info(&self, name: &str) -> Result<Value> {
        let spec = self.spec(name)?;
        let running = self.running(name);
        Ok(json!({
            "name": spec.name,
            "running": running.is_some(),
            "paused": running.as_ref().is_some_and(|r| r.paused.load(std::sync::atomic::Ordering::Relaxed)),
            "ssh_port": running.map(|r| r.ssh_port),
            "ssh_key": self.home.sandbox(name).join("id_ed25519"),
            "workspace": spec.workspace,
            "cpus": spec.cpus,
            "ram_mib": spec.ram_mib,
            "backend": spec.backend,
            "mode": spec.network,
            "rules": spec.rules,
            "secrets": spec.secrets,
            "local_secrets": self.local_secrets(name).list()?.iter().map(|s| json!({ "name": s.name, "hosts": s.hosts, "env": s.env })).collect::<Vec<_>>(),
            "claude_settings": self.claude_settings_file(name).exists(),
            "ports": spec.ports,
            "warnings": self.warnings(&spec),
            "disk_gib": spec.disk_gib,
            "recipes": spec.recipes,
            "job": self.jobs.lock().unwrap().get(name),
        }))
    }

    pub fn list(&self) -> Result<Vec<Value>> {
        let mut names: Vec<String> = std::fs::read_dir(self.home.sandboxes())?
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| self.home.sandbox(name).join("sandbox.toml").exists())
            .collect();
        names.sort();
        names.iter().map(|name| self.info(name)).collect()
    }

    pub fn create(&self, mut spec: SandboxSpec) -> Result<Value> {
        validate_name(&spec.name)?;
        let dir = self.home.sandbox(&spec.name);
        ensure!(!dir.exists(), "sandbox {:?} already exists", spec.name);
        ensure!(spec.cpus >= 1 && spec.ram_mib >= 256, "a sandbox needs at least 1 CPU and 256 MiB of memory");
        if let Some(workspace) = &spec.workspace {
            let path = std::fs::canonicalize(workspace).with_context(|| format!("workspace {} not found", workspace.display()))?;
            ensure!(path.is_dir(), "workspace {} is not a directory", path.display());
            spec.workspace = Some(path);
        }
        for name in &spec.secrets {
            ensure!(self.secrets.get(name)?.is_some(), "secret {name:?} is not set; run `boxquell secret set {name}` first");
        }
        spec.save(&dir)?;
        self.info(&spec.name)
    }

    pub async fn remove(self: &Arc<Self>, name: &str) -> Result<()> {
        self.spec(name)?;
        self.stop(name).await?;
        let _guard = self.lifecycle.lock().await;
        std::fs::remove_dir_all(self.home.sandbox(name))?;
        self.store.delete_sandbox(name)
    }

    /// Load and connection counts of every running sandbox, by name.
    pub fn stats(&self) -> Value {
        use std::sync::atomic::Ordering;
        let running = self.running.lock().unwrap();
        let stats: serde_json::Map<String, Value> = running
            .iter()
            .map(|(name, r)| {
                let samples: Vec<Sample> = r.samples.lock().unwrap().iter().cloned().collect();
                (
                    name.clone(),
                    json!({ "samples": samples, "allowed": r.allowed.load(Ordering::Relaxed), "blocked": r.blocked.load(Ordering::Relaxed) }),
                )
            })
            .collect();
        stats.into()
    }

    /// Whether the emergency stop is on. It survives a restart of the daemon.
    pub fn lockdown(&self) -> bool {
        self.home.root().join("lockdown").exists()
    }

    /// Emergency stop: block every connection of every sandbox at once and
    /// end those that are open, or lift that again.
    pub fn set_lockdown(&self, on: bool) -> Result<()> {
        let file = self.home.root().join("lockdown");
        if on {
            std::fs::write(&file, "")?;
        } else if file.exists() {
            std::fs::remove_file(&file)?;
        }
        for running in self.running.lock().unwrap().values() {
            running.policy.write().unwrap().lockdown = on;
        }
        Ok(())
    }

    /// User preferences of the daemon and the UI.
    pub fn settings(&self) -> Settings {
        std::fs::read(self.home.root().join("settings.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn set_settings(&self, settings: &Settings) -> Result<()> {
        Ok(std::fs::write(self.home.root().join("settings.json"), serde_json::to_vec_pretty(settings)?)?)
    }

    /// Show a desktop notification for a blocked connection, at most once a minute per destination.
    fn notify_blocked(&self, sandbox: &str, event: &boxquell_net::NetEvent) {
        if !self.settings().notify_blocked || self.lockdown() {
            return;
        }
        let target = event.host.clone().or_else(|| event.ip.map(|ip| ip.to_string())).unwrap_or_else(|| "unknown".into());
        {
            let mut seen = self.notified.lock().unwrap();
            let now = std::time::Instant::now();
            seen.retain(|_, at| now.duration_since(*at) < NOTIFY_QUIET);
            if seen.insert((sandbox.to_string(), target.clone()), now).is_some() {
                return;
            }
        }
        let port =
            event.port.filter(|_| event.host.is_none() || event.kind != boxquell_net::EventKind::Dns).map(|p| format!(":{p}")).unwrap_or_default();
        crate::notify::show(&format!("Boxquell blocked a connection in {sandbox}"), &format!("{target}{port}: {}", event.reason));
    }

    /// What keeps the installed agents from working: missing credentials.
    fn warnings(&self, spec: &SandboxSpec) -> Vec<String> {
        let mut warnings = Vec::new();
        for name in &spec.secrets {
            if !matches!(self.secrets.get(name), Ok(Some(_))) {
                warnings.push(format!("The secret {name} is attached but no longer stored."));
            }
        }
        if spec.recipes.is_empty() {
            return warnings;
        }
        let Ok(catalog) = boxquell_core::recipe::Catalog::of(&self.home) else { return warnings };
        let local = self.local_secrets(&spec.name).list().unwrap_or_default();
        for entry in &spec.recipes {
            let id = entry.split('@').next().unwrap_or(entry);
            let Some(recipe) = catalog.get(id).filter(|recipe| !recipe.secrets.is_empty()) else { continue };
            let shared = recipe.secrets.iter().any(|secret| spec.secrets.contains(secret));
            let own = local.iter().any(|secret| secret.hosts.iter().any(|host| recipe.allow.contains(host)));
            let settings = id.ends_with(":claude") && self.claude_settings_file(&spec.name).exists();
            if !(shared || own || settings) {
                let hint = if id.ends_with(":claude") { ", or give the sandbox Claude settings" } else { "" };
                warnings.push(format!("{} has no credential: store and attach one of [{}]{hint}.", recipe.name, recipe.secrets.join(", ")));
            }
        }
        warnings
    }

    /// Rules that apply to every sandbox.
    pub fn global_rules(&self) -> Vec<Rule> {
        std::fs::read(self.home.root().join("rules.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    /// Global denies come first so no sandbox can undo them; global allows
    /// come last so a sandbox can still deny what is allowed everywhere.
    fn effective_rules(&self, spec: &SandboxSpec) -> Vec<Rule> {
        let global = self.global_rules();
        let (denies, allows): (Vec<Rule>, Vec<Rule>) = global.into_iter().partition(|rule| rule.action == boxquell_net::Action::Deny);
        denies.into_iter().chain(spec.rules.iter().cloned()).chain(allows).collect()
    }

    /// Change the global rules and push the result into every running sandbox.
    pub fn update_global_rules(&self, change: impl FnOnce(&mut Vec<Rule>)) -> Result<Vec<Rule>> {
        let mut rules = self.global_rules();
        change(&mut rules);
        std::fs::write(self.home.root().join("rules.json"), serde_json::to_vec_pretty(&rules)?)?;
        let running: Vec<(String, Arc<Running>)> = self.running.lock().unwrap().iter().map(|(n, r)| (n.clone(), r.clone())).collect();
        for (name, running) in running {
            running.policy.write().unwrap().rules = self.effective_rules(&self.spec(&name)?);
        }
        Ok(rules)
    }

    /// Secrets that belong to one sandbox only, kept in its directory.
    pub fn local_secrets(&self, name: &str) -> SecretStore {
        SecretStore::new(&self.home.sandbox(name).join("secrets.json"))
    }

    /// The sandbox's own secrets plus the shared ones attached to it. Its own win on equal names.
    fn sandbox_secrets(&self, spec: &SandboxSpec) -> Result<Vec<SecretDef>> {
        let mut all = self.local_secrets(&spec.name).list()?;
        for name in &spec.secrets {
            if !all.iter().any(|secret| secret.name == *name) {
                all.push(self.secrets.get(name)?.with_context(|| format!("secret {name:?} is not set; run `boxquell secret set {name}`"))?);
            }
        }
        Ok(all)
    }

    pub fn set_local_secret(&self, name: &str, secret: SecretDef) -> Result<Value> {
        self.spec(name)?;
        self.local_secrets(name).set(secret)?;
        self.refresh_secrets()?;
        self.info(name)
    }

    pub fn remove_local_secret(&self, name: &str, secret: &str) -> Result<Value> {
        self.spec(name)?;
        self.local_secrets(name).remove(secret)?;
        self.refresh_secrets()?;
        self.info(name)
    }

    fn claude_settings_file(&self, name: &str) -> PathBuf {
        self.home.sandbox(name).join("claude-managed-settings.json")
    }

    /// Give the sandbox Claude Code settings (see `boxquell_core::claude`). Credentials
    /// in them become secrets of this sandbox; the guest gets placeholders.
    pub fn set_claude_settings(&self, name: &str, text: &str) -> Result<Value> {
        self.spec(name)?;
        let imported = boxquell_core::claude::import(text)?;
        boxquell_core::write_private(&self.claude_settings_file(name), imported.settings.as_bytes())?;
        for secret in imported.secrets {
            self.local_secrets(name).set(secret)?;
        }
        self.refresh_secrets()?;
        let rules: Vec<Rule> = imported.rules.iter().map(|rule| rule.parse().map_err(anyhow::Error::msg)).collect::<Result<_>>()?;
        self.update_rules(name, |current| {
            for rule in rules {
                if !current.contains(&rule) {
                    current.insert(0, rule);
                }
            }
        })?;
        let mut info = self.info(name)?;
        info["notes"] = imported.notes.into();
        Ok(info)
    }

    pub fn clear_claude_settings(&self, name: &str) -> Result<Value> {
        self.spec(name)?;
        let _ = std::fs::remove_file(self.claude_settings_file(name));
        self.info(name)
    }

    /// Change CPUs, memory or disk size. A running sandbox uses them after its next start.
    pub fn resize(&self, name: &str, cpus: Option<u8>, ram_mib: Option<u32>, disk_gib: Option<u32>) -> Result<Value> {
        let mut spec = self.spec(name)?;
        if let Some(cpus) = cpus {
            ensure!(cpus >= 1, "a sandbox needs at least 1 CPU");
            spec.cpus = cpus;
        }
        if let Some(ram_mib) = ram_mib {
            ensure!(ram_mib >= 256, "a sandbox needs at least 256 MiB of memory");
            spec.ram_mib = ram_mib;
        }
        if let Some(disk_gib) = disk_gib {
            ensure!(disk_gib >= spec.disk_gib, "a disk can only grow: {name} has {} GiB", spec.disk_gib);
            spec.disk_gib = disk_gib;
        }
        spec.save(&self.home.sandbox(name))?;
        self.info(name)
    }

    /// Forward a port of this computer into the sandbox, now and on every start.
    pub async fn add_port(&self, name: &str, port: PortForward) -> Result<Value> {
        let mut spec = self.spec(name)?;
        ensure!(port.host >= 1024 && port.guest >= 1, "use a port of this computer from 1024 up");
        ensure!(!spec.ports.iter().any(|p| p.host == port.host), "port {} of this computer already leads into {name}", port.host);
        if let Some(running) = self.running(name) {
            open_forward(&running, port).await?;
        }
        spec.ports.push(port);
        spec.save(&self.home.sandbox(name))?;
        self.info(name)
    }

    pub fn remove_port(&self, name: &str, host: u16) -> Result<Value> {
        let mut spec = self.spec(name)?;
        spec.ports.retain(|p| p.host != host);
        spec.save(&self.home.sandbox(name))?;
        if let Some(task) = self.running(name).and_then(|running| running.forwards.lock().unwrap().remove(&host)) {
            task.abort();
        }
        self.info(name)
    }

    /// Change a sandbox's rules on disk and, if it runs, in its gateway.
    pub fn update_rules(&self, name: &str, change: impl FnOnce(&mut Vec<Rule>)) -> Result<Value> {
        let mut spec = self.spec(name)?;
        change(&mut spec.rules);
        spec.save(&self.home.sandbox(name))?;
        if let Some(running) = self.running(name) {
            running.policy.write().unwrap().rules = self.effective_rules(&spec);
        }
        self.info(name)
    }

    /// Freeze or thaw a running sandbox. A paused VM keeps its memory and
    /// open programs but uses no CPU; its network connections may time out.
    pub fn set_paused(&self, name: &str, paused: bool) -> Result<Value> {
        use std::sync::atomic::Ordering;
        let running = self.running(name).with_context(|| format!("{name} is not running"))?;
        if running.paused.load(Ordering::Relaxed) != paused {
            #[cfg(unix)]
            {
                let pid = running.pid.context("the VM process is gone")?;
                let signal = if paused { "-STOP" } else { "-CONT" };
                let status = std::process::Command::new("kill").args([signal, &pid.to_string()]).status().context("cannot run kill")?;
                ensure!(status.success(), "cannot signal the VM process");
                running.paused.store(paused, Ordering::Relaxed);
            }
            #[cfg(not(unix))]
            anyhow::bail!("pausing is not supported on this system yet");
        }
        self.info(name)
    }

    /// Switch between whitelist and blacklist; a running sandbox follows at once.
    pub fn set_mode(&self, name: &str, mode: boxquell_net::Mode) -> Result<Value> {
        let mut spec = self.spec(name)?;
        spec.network = mode;
        spec.save(&self.home.sandbox(name))?;
        if let Some(running) = self.running(name) {
            running.policy.write().unwrap().mode = mode;
        }
        self.info(name)
    }

    /// Remember that recipes were installed, for listings and the UI.
    pub fn record_recipes(&self, name: &str, ids: &[String]) -> Result<Value> {
        let mut spec = self.spec(name)?;
        for id in ids {
            // One entry per recipe: a newly installed version replaces the old one.
            let name = id.split('@').next().unwrap_or(id);
            spec.recipes.retain(|old| old.split('@').next() != Some(name));
            spec.recipes.push(id.clone());
        }
        spec.save(&self.home.sandbox(name))?;
        self.info(name)
    }

    /// Install recipes in the background by running `boxquell add`; progress is in `info().job`.
    pub fn start_install(self: &Arc<Self>, name: &str, ids: Vec<String>) -> Result<Value> {
        self.spec(name)?;
        ensure!(!ids.is_empty(), "choose at least one recipe");
        boxquell_core::recipe::Catalog::of(&self.home)?.resolve(&ids)?;
        {
            let mut jobs = self.jobs.lock().unwrap();
            ensure!(jobs.get(name).is_none_or(|job| job.state != "running"), "an install is already running in {name}");
            jobs.insert(name.to_string(), Job { state: "running", recipes: ids.clone(), log: String::new() });
        }
        let mut child = tokio::process::Command::new(EXE.get().context("daemon executable path is unknown")?)
            .arg("add")
            .arg(name)
            .args(&ids)
            .env("BOXQUELL_HOME", self.home.root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot start the installer")?;
        let sandbox = name.to_string();
        let (daemon, name) = (self.clone(), name.to_string());
        let append = |daemon: Arc<Daemon>, name: String, mut pipe: Box<dyn AsyncRead + Unpin + Send>| async move {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = pipe.read(&mut buf).await {
                if let Some(job) = daemon.jobs.lock().unwrap().get_mut(&name) {
                    job.log.push_str(&String::from_utf8_lossy(&buf[..n]));
                    // Keep the tail: the end of an install log is what matters.
                    if job.log.len() > 64_000 {
                        let cut = job.log.char_indices().map(|(i, _)| i).find(|i| *i >= job.log.len() - 48_000).unwrap_or(0);
                        job.log.drain(..cut);
                    }
                }
            }
        };
        let out = tokio::spawn(append(daemon.clone(), name.clone(), Box::new(child.stdout.take().context("no stdout")?)));
        let err = tokio::spawn(append(daemon.clone(), name.clone(), Box::new(child.stderr.take().context("no stderr")?)));
        tokio::spawn(async move {
            let ok = child.wait().await.is_ok_and(|status| status.success());
            let _ = tokio::join!(out, err);
            if let Some(job) = daemon.jobs.lock().unwrap().get_mut(&name) {
                job.state = if ok { "done" } else { "failed" };
            }
        });
        self.info(&sandbox)
    }

    /// Give a sandbox access to a host secret. The placeholder variable shows up after a restart.
    pub fn attach_secret(&self, name: &str, secret: &str) -> Result<Value> {
        ensure!(self.secrets.get(secret)?.is_some(), "secret {secret:?} is not set; run `boxquell secret set {secret}` first");
        let mut spec = self.spec(name)?;
        if !spec.secrets.iter().any(|s| s == secret) {
            spec.secrets.push(secret.to_string());
            spec.save(&self.home.sandbox(name))?;
        }
        self.refresh_secrets()?;
        self.info(name)
    }

    /// Push the current secret values into every running gateway.
    pub fn refresh_secrets(&self) -> Result<()> {
        let running: Vec<(String, Arc<Running>)> = self.running.lock().unwrap().iter().map(|(n, r)| (n.clone(), r.clone())).collect();
        for (name, running) in running {
            let spec = self.spec(&name)?;
            let defs = self.sandbox_secrets(&spec).unwrap_or_default();
            *running.secrets.write().unwrap() = defs.iter().map(net_secret).collect();
        }
        Ok(())
    }

    pub async fn start(self: &Arc<Self>, name: &str) -> Result<Value> {
        let _guard = self.lifecycle.lock().await;
        let spec = self.spec(name)?;
        if self.running(name).is_some() {
            // Whoever asks for a running sandbox wants to use it.
            self.set_paused(name, false)?;
            return self.info(name);
        }
        let dir = self.home.sandbox(name);
        let base = self.home.base_image();

        let disk = dir.join("disk.img");
        if !disk.exists() {
            ensure!(base.exists(), "base image missing: build it with `image/build.sh {}`", base.display());
            let (from, to, tmp) = (base.clone(), disk.clone(), dir.join("disk.img.partial"));
            tokio::task::spawn_blocking(move || sparse_copy(&from, &tmp).and_then(|()| std::fs::rename(&tmp, &to)))
                .await?
                .context("cannot create the sandbox disk")?;
        }

        let script_disk = disk.clone();
        tokio::task::spawn_blocking(move || sync_guest_scripts(&script_disk)).await?;

        // Growing is sparse and costs no space until the guest writes to it.
        let wanted = u64::from(spec.disk_gib) << 30;
        let file = std::fs::OpenOptions::new().write(true).open(&disk).context("cannot open the sandbox disk")?;
        if file.metadata()?.len() < wanted {
            file.set_len(wanted).context("cannot grow the sandbox disk")?;
        }
        drop(file);

        let key = dir.join("id_ed25519");
        if !key.exists() {
            let status = tokio::process::Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", &format!("boxquell-{name}"), "-f"])
                .arg(&key)
                .status()
                .await
                .context("cannot run ssh-keygen; is OpenSSH installed?")?;
            ensure!(status.success(), "ssh-keygen failed");
        }
        let public_key = std::fs::read(key.with_extension("pub"))?;

        // The guest sees placeholders only; the gateway holds the real values.
        let secrets = self.sandbox_secrets(&spec)?;
        let mut env = String::from("# Generated by boxquell at every boot.\n");
        let ca_bundle = "/etc/ssl/certs/ca-certificates.crt";
        let mut export = |key: &str, value: &str| env.push_str(&format!("export {key}={}\n", shell_quote(value)));
        export("NODE_EXTRA_CA_CERTS", "/run/boxquell/ca.crt");
        for var in ["SSL_CERT_FILE", "CODEX_CA_CERTIFICATE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"] {
            export(var, ca_bundle);
        }
        for secret in &secrets {
            for var in &secret.env {
                export(var, &secret.placeholder);
            }
        }
        for (key, value) in &spec.env {
            export(key, value);
        }
        env.push_str("[ \"$PWD\" = \"$HOME\" ] && [ -d /workspace ] && cd /workspace\n");
        let claude_settings = std::fs::read(self.claude_settings_file(name)).ok();
        let mut drive_files = vec![
            DriveFile { path: "ca.crt", contents: self.ca.cert_pem().as_bytes(), mode: 0o644 },
            DriveFile { path: "authorized_keys", contents: &public_key, mode: 0o600 },
            DriveFile { path: "env.sh", contents: env.as_bytes(), mode: 0o644 },
        ];
        if let Some(settings) = &claude_settings {
            drive_files.push(DriveFile { path: "claude-managed-settings.json", contents: settings, mode: 0o644 });
        }
        let config_drive = dir.join("config.tar");
        drive::write(&config_drive, &drive_files)?;

        let (listener, net) = LinkListener::bind(&dir).await?;
        let vm = VmSpec { cpus: spec.cpus, ram_mib: spec.ram_mib, disk, config_drive, net, workspace: spec.workspace.clone() };
        let command = match spec.backend {
            #[cfg(unix)]
            Backend::Krun => {
                let library = boxquell_vm::krun::find_library(&self.home.root().join("lib"));
                boxquell_vm::krun::probe(&library)?;
                boxquell_vm::krun::command(EXE.get().context("daemon executable path is unknown")?, &library, &serde_json::to_string(&vm)?)
            }
            #[cfg(not(unix))]
            Backend::Krun => anyhow::bail!("the libkrun backend is not available on this system; use the qemu backend"),
            Backend::Qemu => {
                let (kernel, initrd) = (base.with_extension("vmlinuz"), base.with_extension("initrd"));
                ensure!(kernel.exists() && initrd.exists(), "kernel or initrd missing next to {}", base.display());
                boxquell_vm::qemu::command(&vm, &kernel, &initrd)
            }
        };
        let console = dir.join("console.log");
        let log = std::fs::File::create(&console)?;
        let mut command = tokio::process::Command::from(command);
        command.stdin(Stdio::null()).stdout(log.try_clone()?).stderr(log).kill_on_drop(true);
        let mut child = command.spawn().context("cannot start the VM process")?;

        let failed = |what: &str| anyhow::anyhow!("{what}; last console output:\n{}", tail(&console, 15));
        let link = tokio::select! {
            accepted = tokio::time::timeout(BOOT_TIMEOUT, listener.accept()) => match accepted {
                Ok(link) => link?,
                Err(_) => {
                    let _ = child.kill().await;
                    return Err(failed("the VM did not connect its network in time"));
                }
            },
            status = child.wait() => return Err(failed(&format!("the VM exited during start ({})", status?))),
        };

        let policy = Arc::new(RwLock::new(Policy { mode: spec.network, rules: self.effective_rules(&spec), lockdown: self.lockdown() }));
        let live_secrets = Arc::new(RwLock::new(secrets.iter().map(net_secret).collect::<Vec<_>>()));
        let intercept = Intercept { ca: self.ca.clone(), secrets: live_secrets.clone(), log_all_requests: false, roots: self.roots.clone() };
        let (events_tx, mut events) = mpsc::unbounded_channel();
        let gateway = boxquell_net::spawn(link, SystemUpstream, policy.clone(), Some(intercept), events_tx);
        let (daemon, sandbox) = (self.clone(), name.to_string());
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if !event.allowed {
                    daemon.notify_blocked(&sandbox, &event);
                }
                // A closing connection was already counted when it opened.
                if event.phase != boxquell_net::Phase::Close {
                    if let Some(running) = daemon.running(&sandbox) {
                        let counter = if event.allowed { &running.allowed } else { &running.blocked };
                        counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                }
                match daemon.store.insert(&sandbox, &event) {
                    Ok(stored) => drop(daemon.live.send(stored)),
                    Err(err) => eprintln!("boxquell: cannot store event: {err:#}"),
                }
            }
        });

        let ssh_listener = TcpListener::bind("127.0.0.1:0").await?;
        let ssh_port = ssh_listener.local_addr()?.port();
        let forward = tokio::spawn(forward_port(ssh_listener, gateway.clone(), 22));

        let stop = Arc::new(Notify::new());
        let (exited_tx, exited) = watch::channel(false);
        let running = Arc::new(Running {
            policy,
            secrets: live_secrets,
            ssh_port,
            gateway: gateway.clone(),
            forwards: Mutex::default(),
            samples: Mutex::default(),
            allowed: 0.into(),
            blocked: 0.into(),
            pid: child.id(),
            paused: false.into(),
            stop: stop.clone(),
            exited,
        });
        self.running.lock().unwrap().insert(name.to_string(), running.clone());
        let sampler = tokio::spawn(sample_load(running.clone(), dir.join("id_ed25519")));
        for port in &spec.ports {
            // A port taken by another program must not keep the sandbox from starting.
            if let Err(err) = open_forward(&running, *port).await {
                eprintln!("boxquell: {name}: {err:#}");
            }
        }
        let (daemon, sandbox) = (self.clone(), name.to_string());
        let ending = running.clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = child.wait() => {}
                _ = stop.notified() => {
                    let _ = child.kill().await;
                }
            }
            forward.abort();
            sampler.abort();
            for (_, task) in ending.forwards.lock().unwrap().drain() {
                task.abort();
            }
            daemon.running.lock().unwrap().remove(&sandbox);
            #[cfg(unix)]
            let _ = std::fs::remove_file(net_socket(&daemon.home.sandbox(&sandbox)));
            let _ = exited_tx.send(true);
        });

        // Ready means sshd answers; until then nothing useful can be done with the sandbox.
        let deadline = tokio::time::Instant::now() + BOOT_TIMEOUT;
        loop {
            if *running.exited.borrow() {
                return Err(failed("the VM exited during boot"));
            }
            let banner = async {
                let mut stream = gateway.connect_guest(22).await.ok()?;
                let mut banner = [0u8; 4];
                stream.read_exact(&mut banner).await.ok()?;
                (&banner == b"SSH-").then_some(())
            };
            if let Ok(Some(())) = tokio::time::timeout(Duration::from_secs(1), banner).await {
                break;
            }
            if tokio::time::Instant::now() > deadline {
                running.stop.notify_one();
                return Err(failed("the guest did not start its SSH server in time"));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        self.info(name)
    }

    pub async fn stop(self: &Arc<Self>, name: &str) -> Result<()> {
        let _guard = self.lifecycle.lock().await;
        let Some(running) = self.running(name) else { return Ok(()) };
        // A frozen guest cannot shut itself down.
        self.set_paused(name, false)?;
        let mut exited = running.exited.clone();

        // Ask the guest to flush its disk and leave; fall back to killing the VM process.
        let key = self.home.sandbox(name).join("id_ed25519");
        let mut ssh = tokio::process::Command::new("ssh");
        ssh.args(ssh::base_args(&key, running.ssh_port))
            .args(["sudo", "/usr/local/sbin/boxquell-shutdown"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        let graceful = async {
            let _ = ssh.status().await;
            let _ = exited.wait_for(|done| *done).await;
        };
        if tokio::time::timeout(SHUTDOWN_TIMEOUT, graceful).await.is_err() {
            running.stop.notify_one();
            let mut exited = running.exited.clone();
            let _ = exited.wait_for(|done| *done).await;
        }
        Ok(())
    }

    pub async fn stop_all(self: &Arc<Self>) {
        let names: Vec<String> = self.running.lock().unwrap().keys().cloned().collect();
        for name in names {
            let _ = self.stop(&name).await;
        }
    }
}

async fn open_forward(running: &Running, port: PortForward) -> Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port.host)).await.with_context(|| format!("cannot listen on 127.0.0.1:{}", port.host))?;
    let task = tokio::spawn(forward_port(listener, running.gateway.clone(), port.guest));
    if let Some(old) = running.forwards.lock().unwrap().insert(port.host, task) {
        old.abort();
    }
    Ok(())
}

async fn forward_port(listener: TcpListener, gateway: GatewayHandle, guest_port: u16) {
    while let Ok((mut client, _)) = listener.accept().await {
        let gateway = gateway.clone();
        tokio::spawn(async move {
            if let Ok(mut guest) = gateway.connect_guest(guest_port).await {
                let _ = client.set_nodelay(true);
                let _ = tokio::io::copy_bidirectional(&mut client, &mut guest).await;
            }
        });
    }
}

/// What the CLI needs to find and authenticate to a running daemon.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct DaemonFile {
    pub port: u16,
    pub token: String,
    pub pid: u32,
}

/// Scripts every guest needs, as they are in this version.
const GUEST_SCRIPTS: [(&str, &str); 2] = [
    ("/usr/local/sbin/boxquell-init", include_str!("../../../image/boxquell-init")),
    ("/usr/local/sbin/boxquell-shutdown", include_str!("../../../image/boxquell-shutdown")),
];

/// Bring the guest's start and stop scripts on a stopped sandbox disk up to
/// this version, so sandboxes created by an older version keep booting.
/// Uses `debugfs` from e2fsprogs, which edits an ext4 image without root.
/// Without it nothing is changed and the disk boots with what it has.
fn sync_guest_scripts(disk: &Path) {
    let debugfs =
        |args: &[&std::ffi::OsStr]| std::process::Command::new("debugfs").args(args).arg(disk).stdin(Stdio::null()).stderr(Stdio::null()).output();
    let Ok(dir) = tempdir_for(disk) else { return };
    let mut commands = String::new();
    for (index, (path, contents)) in GUEST_SCRIPTS.iter().enumerate() {
        let current = debugfs(&["-R".as_ref(), format!("cat {path}").as_ref()]);
        match current {
            Ok(output) if output.stdout == contents.as_bytes() => continue,
            Ok(_) => {}
            // debugfs is not installed.
            Err(_) => return,
        }
        let source = dir.join(format!("script-{index}"));
        if std::fs::write(&source, contents).is_err() {
            return;
        }
        commands += &format!("rm {path}\nwrite {} {path}\nsif {path} mode 0100755\n", source.display());
    }
    if !commands.is_empty() {
        let list = dir.join("commands");
        if std::fs::write(&list, commands).is_ok() {
            let _ = debugfs(&["-w".as_ref(), "-f".as_ref(), list.as_os_str()]);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A scratch directory next to the disk.
fn tempdir_for(disk: &Path) -> std::io::Result<PathBuf> {
    let dir = disk.with_file_name("guest-scripts.tmp");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// Where the VM connects its network device. Unix socket paths are limited to
/// about 100 bytes, so a deep data directory falls back to the runtime directory.
#[cfg(unix)]
fn net_socket(sandbox_dir: &Path) -> PathBuf {
    let path = sandbox_dir.join("net.sock");
    if path.as_os_str().len() < 100 {
        return path;
    }
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let name = sandbox_dir.file_name().unwrap_or_default().to_string_lossy();
    base.join(format!("boxquell-{}-{name}.sock", std::process::id()))
}

/// This binary's path, read at startup: after an upgrade replaces the file,
/// `current_exe` would point at the deleted old one.
static EXE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub async fn serve(home: Home) -> Result<()> {
    EXE.get_or_init(|| std::env::current_exe().unwrap_or_else(|_| PathBuf::from("boxquell")));
    let daemon = Daemon::new(home)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let file = DaemonFile { port: listener.local_addr()?.port(), token: daemon.token.clone(), pid: std::process::id() };
    let path: PathBuf = daemon.home.daemon_file();
    boxquell_core::write_private(&path, &serde_json::to_vec(&file)?)?;
    eprintln!("boxquell daemon listening on 127.0.0.1:{}", file.port);

    let app = crate::api::router(daemon.clone());
    let stopper = daemon.clone();
    let shutdown = async move {
        #[cfg(unix)]
        let terminate = async {
            let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("signal handler");
            signal.recv().await;
        };
        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate => {}
            _ = stopper.shutdown.notified() => {}
        }
    };
    let served = axum::serve(listener, app).with_graceful_shutdown(shutdown).await;
    daemon.stop_all().await;
    let _ = std::fs::remove_file(&path);
    served.context("daemon API failed")
}
