//! The per-user background process. It owns every running sandbox: the VM
//! process, its network gateway, the SSH forward, and the event log.

use crate::ssh;
use anyhow::{ensure, Context, Result};
use boxquell_core::drive::{self, shell_quote, DriveFile};
use boxquell_core::spec::{validate_name, SshKeys};
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
    /// When work was last seen in the sandbox; see the idle stop.
    busy_at: Mutex<std::time::Instant>,
    /// The network is cut off because a data limit was reached.
    cut: std::sync::atomic::AtomicBool,
    /// The totals at the last note of traffic for the statistics.
    traffic_noted: Mutex<(u64, u64)>,
    /// Where counting towards the data limits starts: the totals when the
    /// sandbox started or was last let continue.
    traffic_base: Mutex<(u64, u64)>,
    /// Keeps the guest's SSH agent socket connected to this computer's keys.
    key_agent: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// The VM process, for pausing.
    #[cfg_attr(not(unix), allow(dead_code))]
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
/// memory (total, available, kB), root disk (total, used, kB), network bytes (received, sent),
/// and how many terminals are open in the guest.
const SAMPLER: &str = r#"while :; do read -r _ u n s i w q sq st _ < /proc/stat; mt=0; ma=0; while read -r k v _; do case $k in MemTotal:) mt=$v;; MemAvailable:) ma=$v;; esac; done < /proc/meminfo; set -- $(df -k / | tail -n1); rx=0; tx=0; for d in /sys/class/net/e*; do [ -r "$d/statistics/rx_bytes" ] && rx=$(cat "$d/statistics/rx_bytes") && tx=$(cat "$d/statistics/tx_bytes"); done; p=0; for t in /dev/pts/[0-9]*; do [ -e "$t" ] && p=$((p+1)); done; echo "$((u+n+s+i+w+q+sq+st)) $((i+w)) $mt $ma $2 $3 $rx $tx $p"; sleep 2; done"#;

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
            let mut previous: Option<[u64; 9]> = None;
            let mut outside = running.gateway.traffic();
            if let Some(stdout) = child.stdout.take() {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    let numbers: Vec<u64> = line.split_whitespace().filter_map(|n| n.parse().ok()).collect();
                    let Ok(now) = <[u64; 9]>::try_from(numbers) else { continue };
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
                        // Work in the sandbox: something computes, a terminal is open, or data moves to or from outside.
                        let traffic = running.gateway.traffic();
                        if sample.cpu >= 5.0 || now[8] > 0 || traffic != outside {
                            *running.busy_at.lock().unwrap() = std::time::Instant::now();
                        }
                        outside = traffic;
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
    /// Destinations nobody wants to hear about again: no notification and no
    /// entry in the notification list when they are blocked. `*.example.com` works.
    pub ignore_blocked: Vec<String>,
    /// Whether web browsers may open the interface. Off by default: the
    /// desktop app is the interface, and it identifies itself to the daemon.
    /// This is a switch, not a lock; the API token is what protects access.
    pub browser_ui: bool,
    /// Addresses of this computer, besides 127.0.0.1, on which the UI and API
    /// are reachable. Traffic there is not encrypted. Read when the daemon starts.
    pub listen: Vec<std::net::IpAddr>,
    /// The port used on those addresses.
    pub listen_port: u16,
    /// Fetch the newest state of the recipe and system repositories once a day.
    pub update_repos: bool,
    /// Ask GitHub once a day whether a newer Boxquell has been released.
    pub check_updates: bool,
    /// Stop a sandbox after this many minutes without work in it; 0 for never.
    /// A sandbox can have its own value.
    pub idle_stop_minutes: u64,
    /// What assistants connected through `boxquell mcp` may do beyond the
    /// usual: nothing that changes anything; change network rules; delete
    /// sandboxes; take a sandbox's changes in review mode into the project.
    pub mcp_read_only: bool,
    pub mcp_network_rules: bool,
    pub mcp_delete: bool,
    pub mcp_review_apply: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            notify_blocked: true,
            toast_blocked: true,
            ignore_blocked: Vec::new(),
            browser_ui: false,
            listen: Vec::new(),
            listen_port: 7878,
            update_repos: true,
            check_updates: true,
            idle_stop_minutes: 0,
            mcp_read_only: false,
            mcp_network_rules: false,
            mcp_delete: false,
            mcp_review_apply: false,
        }
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
    /// Secrets that come from a helper command: when each was last fetched, by
    /// `<sandbox>/<secret>`, and what went wrong the last time, if anything.
    helper_runs: Mutex<HashMap<String, (std::time::Instant, Option<String>)>>,
    handled_lock: Mutex<()>,
    /// Tasks that run now, by `<sandbox>/<id>`, each with the signal that ends it.
    tasks: Mutex<HashMap<String, Arc<Notify>>>,
    /// Keys whose limit for the day is reached; the gateways refuse requests that would carry one.
    paused_keys: Arc<RwLock<Vec<String>>>,
    /// Recent load of this computer itself, oldest first: (CPU percent, memory used and total in MiB, disk used and total in GiB).
    host_load: Mutex<std::collections::VecDeque<[f64; 5]>>,
    /// What the last look for a newer release found.
    pub release: Mutex<Option<Value>>,
    jobs: Mutex<HashMap<String, Job>>,
    /// Latest image build per operating system.
    os_jobs: Mutex<HashMap<String, Job>>,
    /// Serializes start, stop and removal.
    lifecycle: tokio::sync::Mutex<()>,
}

/// Copy a file, leaving all-zero blocks as holes so images stay small on disk.
/// Copy a disk image. Where the file system can share the data between both
/// files (btrfs, XFS, APFS) that takes a moment and no space; elsewhere the
/// file is copied, leaving out its empty stretches.
fn copy_disk(from: &Path, to: &Path) -> std::io::Result<()> {
    let _ = std::fs::remove_file(to);
    let cloned = if cfg!(target_os = "linux") {
        // Shares the data where it can, and otherwise skips the empty stretches without reading them.
        std::process::Command::new("cp")
            .args(["--reflink=auto", "--sparse=always", "--"])
            .arg(from)
            .arg(to)
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    } else if cfg!(target_os = "macos") {
        std::process::Command::new("cp").arg("-c").arg(from).arg(to).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
    } else {
        false
    };
    if cloned {
        return Ok(());
    }
    let _ = std::fs::remove_file(to);
    sparse_copy(from, to)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// The records (`*.json`) in a directory of snapshots or templates, oldest first.
fn records(dir: &Path) -> Vec<Value> {
    let mut all: Vec<Value> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .filter_map(|path| serde_json::from_slice(&std::fs::read(path).ok()?).ok())
        .collect();
    all.sort_by_key(|record: &Value| record["created_ms"].as_u64().unwrap_or(0));
    all
}

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

/// What every session in the guest starts with: certificates, placeholders
/// for the secrets, the user's variables and the project root.
fn guest_env(spec: &SandboxSpec, secrets: &[SecretDef]) -> String {
    let mut env = String::from("# Generated by boxquell.\n");
    let ca_bundle = "/etc/ssl/certs/ca-certificates.crt";
    let mut export = |key: &str, value: &str| env.push_str(&format!("export {key}={}\n", shell_quote(value)));
    export("NODE_EXTRA_CA_CERTS", "/run/boxquell/ca.crt");
    for var in ["SSL_CERT_FILE", "CODEX_CA_CERTIFICATE", "REQUESTS_CA_BUNDLE", "CURL_CA_BUNDLE"] {
        export(var, ca_bundle);
    }
    for secret in secrets {
        for var in &secret.env {
            export(var, &secret.placeholder);
        }
    }
    // Git over SSH signs through a socket that leads to keys on this computer.
    // A host met for the first time is accepted: no one is there to answer the question.
    export("SSH_AUTH_SOCK", crate::keyagent::GUEST_SOCKET);
    export("GIT_SSH_COMMAND", "ssh -o StrictHostKeyChecking=accept-new");
    for (key, value) in &spec.env {
        export(key, value);
    }
    // Sessions start in the project root.
    let workdir = shell_quote(spec.workdir.as_deref().unwrap_or("/workspace"));
    env.push_str(&format!("[ \"$PWD\" = \"$HOME\" ] && [ -d {workdir} ] && cd {workdir}\n"));
    env
}

/// One source of destinations that open by themselves: its id, its name, and
/// each destination with whether it is on.
pub type AutoRules = (String, String, Vec<(String, bool)>);

/// Run a secret's helper command on this computer and return what it prints.
fn run_helper(command: &str) -> Result<String> {
    let mut shell = if cfg!(windows) { std::process::Command::new("cmd") } else { std::process::Command::new("/bin/sh") };
    shell.arg(if cfg!(windows) { "/C" } else { "-c" }).arg(command).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = shell.spawn().context("the helper could not be started")?;
    // A helper that waits for someone, e.g. for a sign-in in a browser, must not hold everything up.
    let started = std::time::Instant::now();
    while child.try_wait()?.is_none() {
        if started.elapsed() > Duration::from_secs(30) {
            let _ = child.kill();
            let _ = child.wait();
            anyhow::bail!("it did not finish within 30 seconds");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let output = child.wait_with_output()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || value.is_empty() {
        let errors = String::from_utf8_lossy(&output.stderr);
        let last = errors.lines().rev().find(|line| !line.trim().is_empty()).unwrap_or("it printed nothing");
        anyhow::bail!("{}", last.trim());
    }
    Ok(value)
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
            helper_runs: Mutex::default(),
            handled_lock: Mutex::default(),
            tasks: Mutex::default(),
            paused_keys: Arc::default(),
            host_load: Mutex::default(),
            release: Mutex::default(),
            jobs: Mutex::new(HashMap::new()),
            os_jobs: Mutex::new(HashMap::new()),
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
        let system = boxquell_core::os::definitions(&self.home).into_iter().find(|def| def.id == spec.os);
        // What went out and came in since the sandbox started or was last let continue, against its limits.
        let traffic = {
            let (up, down) = running.as_ref().map_or((0, 0), |r| {
                let (up, down) = r.gateway.traffic();
                let base = *r.traffic_base.lock().unwrap();
                (up.saturating_sub(base.0), down.saturating_sub(base.1))
            });
            json!({
                "up": up, "down": down,
                "upload_limit_mib": spec.upload_limit_mib, "download_limit_mib": spec.download_limit_mib,
                "cut": running.as_ref().is_some_and(|r| r.cut.load(std::sync::atomic::Ordering::Relaxed)),
            })
        };
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
            "os": spec.os,
            "os_name": system.as_ref().map(|def| &def.name),
            "os_icon": system.as_ref().and_then(|def| def.icon.as_ref()),
            "os_icon_color": system.as_ref().and_then(|def| def.icon_color.as_ref()),
            "family": boxquell_core::os::family(&self.home, &spec.os),
            "package_hosts": boxquell_core::os::package_hosts(&self.home, &spec.os),
            "mode": spec.network,
            "rules": spec.rules,
            "secrets": spec.secrets,
            "local_secrets": self.local_secrets(name).list()?.iter().map(|s| json!({ "name": s.name, "hosts": s.hosts, "env": s.env })).collect::<Vec<_>>(),
            "claude_settings": self.claude_settings_file(name).exists(),
            "ports": spec.ports,
            "addresses": json!({ "sandbox": boxquell_net::GUEST_IP, "host": boxquell_net::GATEWAY_IP, "dns": boxquell_net::DNS_IP }),
            "shares": spec.shares,
            "workdir": spec.workdir,
            "autostart": spec.autostart,
            "env": spec.env,
            "ssh": spec.ssh,
            "rule_expiry": spec.rule_expiry,
            "upload_kbit": spec.upload_kbit,
            "download_kbit": spec.download_kbit,
            "idle_stop_minutes": spec.idle_stop_minutes,
            "traffic": traffic,
            "recipe_updates": self.recipe_updates(&spec),
            "review": spec.review,
            "snapshots": self.snapshots(name),
            "tasks": self.tasks(name),
            "auto_rules": self.auto_rules(&spec).into_iter().map(|(source, name, hosts)| json!({
                "source": source,
                "name": name,
                "hosts": hosts.into_iter().map(|(host, enabled)| json!({ "host": host, "enabled": enabled })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "warnings": self.warnings(&spec),
            "disk_gib": spec.disk_gib,
            "recipes": spec.recipes,
            "job": self.jobs.lock().unwrap().get(name),
        }))
    }

    /// The names of all sandboxes, sorted.
    fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.home.sandboxes())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
            .filter(|name| self.home.sandbox(name).join("sandbox.toml").exists())
            .collect();
        names.sort();
        names
    }

    pub fn list(&self) -> Result<Vec<Value>> {
        self.names().iter().map(|name| self.info(name)).collect()
    }

    pub fn create(&self, mut spec: SandboxSpec) -> Result<Value> {
        validate_name(&spec.name)?;
        let dir = self.home.sandbox(&spec.name);
        ensure!(!dir.exists(), "sandbox {:?} already exists", spec.name);
        ensure!(spec.cpus >= 1 && spec.ram_mib >= 256, "a sandbox needs at least 1 CPU and 256 MiB of memory");
        let image = boxquell_core::os::image(&self.home, &spec.os);
        ensure!(image.exists(), "the system {:?} is not on this computer yet; run `boxquell os install {}`", spec.os, spec.os);
        ensure!(
            spec.backend != Backend::Qemu || boxquell_core::os::has_kernel(&image),
            "the system {:?} brings no kernel, so it runs on the krun backend only",
            spec.os
        );
        if let Some(workspace) = &spec.workspace {
            let path = std::fs::canonicalize(workspace).with_context(|| format!("workspace {} not found", workspace.display()))?;
            ensure!(path.is_dir(), "workspace {} is not a directory", path.display());
            spec.workspace = Some(path);
        }
        for name in &spec.secrets {
            ensure!(self.secrets.get(name)?.is_some(), "secret {name:?} is not set; run `boxquell secret set {name}` first");
        }
        // From a template: its system, software and variables, and at the first start its disk.
        let template = spec.template.take();
        if let Some(template) = &template {
            validate_name(template)?;
            let record: Value = std::fs::read(self.templates_dir().join(format!("{template}.json")))
                .ok()
                .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                .with_context(|| format!("there is no template {template:?}"))?;
            spec.os = record["os"].as_str().unwrap_or(&spec.os).to_string();
            spec.recipes = serde_json::from_value(record["recipes"].clone()).unwrap_or_default();
            spec.recipe_revisions = serde_json::from_value(record["recipe_revisions"].clone()).unwrap_or_default();
            for (key, value) in serde_json::from_value::<std::collections::BTreeMap<String, String>>(record["env"].clone()).unwrap_or_default() {
                spec.env.entry(key).or_insert(value);
            }
            spec.disk_gib = spec.disk_gib.max(record["disk_gib"].as_u64().unwrap_or(0) as u32);
        }
        spec.save(&dir)?;
        if spec.review {
            let project = spec.workspace.clone().context("review mode needs a project folder")?;
            let base = crate::review::start(&project, &dir.join("work"))?;
            std::fs::write(dir.join("review-base.json"), serde_json::to_vec(&base)?)?;
        }
        if let Some(template) = template {
            std::fs::write(dir.join("template"), template)?;
        }
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

    /// The latest build of a system's image, if one was started since the daemon came up.
    pub fn os_job(&self, id: &str) -> Option<Job> {
        self.os_jobs.lock().unwrap().get(id).cloned()
    }

    /// Build a system's image in the background by running `boxquell os build`.
    pub fn start_os_build(self: &Arc<Self>, id: &str) -> Result<()> {
        ensure!(boxquell_core::os::definitions(&self.home).iter().any(|def| def.id == id), "no system named {id:?}");
        {
            let mut jobs = self.os_jobs.lock().unwrap();
            if jobs.get(id).is_some_and(|job| job.state == "running") {
                return Ok(());
            }
            jobs.insert(id.to_string(), Job { state: "running", recipes: Vec::new(), log: String::new() });
        }
        let mut child = tokio::process::Command::new(EXE.get().context("daemon executable path is unknown")?)
            .args(["os", "install", "--", id])
            .env("BOXQUELL_HOME", self.home.root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("cannot start the build")?;
        let append = |daemon: Arc<Daemon>, id: String, mut pipe: Box<dyn AsyncRead + Unpin + Send>| async move {
            let mut buf = [0u8; 4096];
            while let Ok(n @ 1..) = pipe.read(&mut buf).await {
                if let Some(job) = daemon.os_jobs.lock().unwrap().get_mut(&id) {
                    job.log.push_str(&String::from_utf8_lossy(&buf[..n]));
                    if job.log.len() > 64_000 {
                        let cut = job.log.char_indices().map(|(i, _)| i).find(|i| *i >= job.log.len() - 48_000).unwrap_or(0);
                        job.log.drain(..cut);
                    }
                }
            }
        };
        let out = tokio::spawn(append(self.clone(), id.to_string(), Box::new(child.stdout.take().context("no stdout")?)));
        let err = tokio::spawn(append(self.clone(), id.to_string(), Box::new(child.stderr.take().context("no stderr")?)));
        let (daemon, id) = (self.clone(), id.to_string());
        tokio::spawn(async move {
            let ok = child.wait().await.is_ok_and(|status| status.success());
            let _ = tokio::join!(out, err);
            if let Some(job) = daemon.os_jobs.lock().unwrap().get_mut(&id) {
                job.state = if ok { "done" } else { "failed" };
            }
        });
        Ok(())
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
            // A sandbox that reached its data limit stays cut off when the emergency stop is lifted.
            running.policy.write().unwrap().lockdown = on || running.cut.load(std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    /// Add a rule to a sandbox, first so that it wins over older ones for the
    /// same destination. With `minutes` it ends by itself.
    pub fn add_rule(&self, name: &str, rule: Rule, minutes: Option<u64>) -> Result<Value> {
        let text = rule.to_string();
        let mut spec = self.spec(name)?;
        let replaced: Vec<String> = spec.rules.iter().filter(|r| r.host == rule.host && r.port == rule.port).map(Rule::to_string).collect();
        for old in replaced {
            spec.rule_expiry.remove(&old);
        }
        match minutes.filter(|minutes| *minutes > 0) {
            Some(minutes) => spec.rule_expiry.insert(text, now_ms() / 1000 + minutes * 60),
            None => spec.rule_expiry.remove(&text),
        };
        spec.save(&self.home.sandbox(name))?;
        self.update_rules(name, |rules| {
            rules.retain(|r| !(r.host == rule.host && r.port == rule.port));
            rules.insert(0, rule);
        })
    }

    /// When an idle sandbox is stopped: minutes, 0 for never, `None` for the setting of all sandboxes.
    pub fn set_idle_stop(&self, name: &str, minutes: Option<u64>) -> Result<()> {
        let mut spec = self.spec(name)?;
        spec.idle_stop_minutes = minutes;
        spec.save(&self.home.sandbox(name))
    }

    /// How fast a sandbox may send and receive, in kbit per second; 0 takes a limit away.
    pub fn set_speed_limits(&self, name: &str, upload_kbit: Option<u64>, download_kbit: Option<u64>) -> Result<()> {
        let mut spec = self.spec(name)?;
        if let Some(limit) = upload_kbit {
            spec.upload_kbit = Some(limit).filter(|limit| *limit > 0);
        }
        if let Some(limit) = download_kbit {
            spec.download_kbit = Some(limit).filter(|limit| *limit > 0);
        }
        spec.save(&self.home.sandbox(name))?;
        if let Some(running) = self.running(name) {
            running.gateway.set_speed(spec.upload_kbit.unwrap_or(0) * 125, spec.download_kbit.unwrap_or(0) * 125);
        }
        Ok(())
    }

    /// A sandbox's traffic over the last `days`, per hour or day, with the destinations that carried most of it.
    pub fn traffic_report(&self, name: &str, days: u64) -> Result<Value> {
        self.spec(name)?;
        let bucket = if days <= 2 { 3_600_000 } else { 86_400_000 };
        self.store.traffic(name, now_ms().saturating_sub(days * 86_400_000), bucket)
    }

    /// The data limits of a sandbox; 0 takes a limit away.
    pub fn set_data_limits(&self, name: &str, upload_mib: Option<u64>, download_mib: Option<u64>) -> Result<()> {
        let mut spec = self.spec(name)?;
        if let Some(limit) = upload_mib {
            spec.upload_limit_mib = Some(limit).filter(|limit| *limit > 0);
        }
        if let Some(limit) = download_mib {
            spec.download_limit_mib = Some(limit).filter(|limit| *limit > 0);
        }
        spec.save(&self.home.sandbox(name))
    }

    /// Let a sandbox that reached a data limit go on: counting starts anew.
    pub fn continue_after_limit(&self, name: &str) -> Result<Value> {
        if let Some(running) = self.running(name) {
            *running.traffic_base.lock().unwrap() = running.gateway.traffic();
            running.cut.store(false, std::sync::atomic::Ordering::Relaxed);
            running.policy.write().unwrap().lockdown = self.lockdown();
        }
        self.info(name)
    }

    /// Every few seconds: end rules whose time is up, and cut off sandboxes that reached a data limit.
    pub async fn keep_rules(self: Arc<Self>) {
        use std::sync::atomic::Ordering::Relaxed;
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let now = now_ms() / 1000;
            for name in self.names() {
                let Ok(mut spec) = self.spec(&name) else { continue };
                let due: Vec<String> = spec.rule_expiry.iter().filter(|(_, until)| **until <= now).map(|(rule, _)| rule.clone()).collect();
                if !due.is_empty() {
                    spec.rule_expiry.retain(|rule, _| !due.contains(rule));
                    if spec.save(&self.home.sandbox(&name)).is_ok() {
                        let _ = self.update_rules(&name, |rules| rules.retain(|rule| !due.contains(&rule.to_string())));
                    }
                }
                let Some(running) = self.running(&name) else { continue };
                // Once a minute, what went out and came in is noted for the statistics.
                if now % 60 < 2 {
                    let total = running.gateway.traffic();
                    let mut noted = running.traffic_noted.lock().unwrap();
                    let (up, down) = (total.0.saturating_sub(noted.0), total.1.saturating_sub(noted.1));
                    if up + down > 0 {
                        let _ = self.store.record_traffic(&name, now * 1000, up, down);
                    }
                    *noted = total;
                }
                // The idle stop: nothing computed, no terminal open and no data moved for long enough.
                // A sandbox that starts servers by itself is left alone unless it has a value of its own.
                let minutes = match spec.idle_stop_minutes {
                    Some(minutes) => minutes,
                    None if spec.autostart.is_empty() => self.settings().idle_stop_minutes,
                    None => 0,
                };
                let in_task = self.tasks.lock().unwrap().keys().any(|key| key.starts_with(&format!("{name}/")));
                let working =
                    in_task || running.paused.load(Relaxed) || self.jobs.lock().unwrap().get(&name).is_some_and(|job| job.state == "running");
                if minutes > 0 && !working && running.busy_at.lock().unwrap().elapsed() >= Duration::from_secs(minutes * 60) {
                    // Counting starts over, so the stop is asked for once.
                    *running.busy_at.lock().unwrap() = std::time::Instant::now();
                    crate::notify::show(
                        &format!("{name} was stopped"),
                        &format!("Nothing happened in it for {minutes} minutes. Its disk and settings stay."),
                    );
                    let (daemon, sandbox) = (self.clone(), name.clone());
                    tokio::spawn(async move {
                        let _ = daemon.stop(&sandbox).await;
                    });
                    continue;
                }
                if running.cut.load(Relaxed) {
                    continue;
                }
                let (up, down) = running.gateway.traffic();
                let base = *running.traffic_base.lock().unwrap();
                let over = |used: u64, limit: Option<u64>| limit.is_some_and(|limit| used >= limit << 20);
                let which = if over(up.saturating_sub(base.0), spec.upload_limit_mib) {
                    "upload"
                } else if over(down.saturating_sub(base.1), spec.download_limit_mib) {
                    "download"
                } else {
                    continue;
                };
                running.cut.store(true, Relaxed);
                running.policy.write().unwrap().lockdown = true;
                crate::notify::show(
                    &format!("{name} reached its {which} limit"),
                    "Its network is cut off. Open the sandbox's Network tab to let it continue.",
                );
            }
        }
    }

    /// User preferences of the daemon and the UI.
    pub fn settings(&self) -> Settings {
        std::fs::read(self.home.root().join("settings.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn set_settings(&self, settings: &Settings) -> Result<()> {
        Ok(std::fs::write(self.home.root().join("settings.json"), serde_json::to_vec_pretty(settings)?)?)
    }

    /// Once a day, as far as the settings allow: fetch the repositories and look
    /// for a newer release. When each last happened survives a restart, so a
    /// daemon that starts often does not ask more often.
    pub async fn keep_current(self: Arc<Self>) {
        const EVERY: u64 = 24 * 60 * 60;
        let file = self.home.root().join("updates.json");
        let now = || std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        // Let the daemon settle first; whoever started it is waiting for a sandbox.
        tokio::time::sleep(Duration::from_secs(60)).await;
        loop {
            let mut state: Value = std::fs::read(&file).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_else(|| json!({}));
            let due = |key: &str, state: &Value| now().saturating_sub(state[key].as_u64().unwrap_or(0)) >= EVERY;
            let settings = self.settings();
            if settings.update_repos && due("repos_at", &state) {
                let home = self.home.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    for repo in boxquell_core::repo::list(&home) {
                        if let Err(err) = boxquell_core::repo::update(&home, &repo.name) {
                            eprintln!("boxquell: updating the repository {}: {err:#}", repo.name);
                        }
                    }
                })
                .await;
                state["repos_at"] = json!(now());
            }
            if settings.check_updates && due("checked_at", &state) {
                if let Ok(Ok(found)) = tokio::task::spawn_blocking(crate::api::latest_release).await {
                    // One notification per release, not one per day.
                    let latest = found["latest"].as_str().unwrap_or_default().to_string();
                    if found["newer"] == true && state["notified"] != json!(latest) {
                        crate::notify::show(&format!("Boxquell {latest} is available"), "Open Settings › About to see what is new.");
                        state["notified"] = json!(latest);
                    }
                    *self.release.lock().unwrap() = Some(found);
                    state["checked_at"] = json!(now());
                }
            }
            let _ = std::fs::write(&file, serde_json::to_vec_pretty(&state).unwrap_or_default());
            tokio::time::sleep(Duration::from_secs(30 * 60)).await;
        }
    }

    /// Show a desktop notification for a blocked connection, at most once a minute per destination.
    fn notify_blocked(&self, sandbox: &str, event: &boxquell_net::NetEvent) {
        if !self.settings().notify_blocked || self.lockdown() {
            return;
        }
        let target = event.host.clone().or_else(|| event.ip.map(|ip| ip.to_string())).unwrap_or_else(|| "unknown".into());
        if self.settings().ignore_blocked.iter().any(|pattern| boxquell_net::policy::host_matches(pattern, &target)) {
            return;
        }
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
        let mut warnings = crate::keyagent::problems(&self.global_ssh_keys().with(&spec.ssh));
        if self.running(&spec.name).is_some_and(|running| running.cut.load(std::sync::atomic::Ordering::Relaxed)) {
            warnings.push("A data limit was reached: the network of this sandbox is cut off until you let it continue under Network.".to_string());
        }
        let prefix = format!("{}/", spec.name);
        for (key, (_, problem)) in self.helper_runs.lock().unwrap().iter() {
            if let (Some(secret), Some(problem)) = (key.strip_prefix(&prefix), problem) {
                warnings.push(format!("The key helper of the secret {secret} failed on this computer: {problem}"));
            }
        }
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
                warnings.push(format!("{} has no credential: add one of [{}] under Config{hint}.", recipe.name, recipe.secrets.join(", ")));
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
    /// What the sandbox's system and installed software open by themselves:
    /// per source its name and its destinations, each with whether it is on.
    /// The system's package servers come first, then the software in the order it was installed.
    pub fn auto_rules(&self, spec: &SandboxSpec) -> Vec<AutoRules> {
        let on = |source: &str, host: &str| !spec.auto_off.iter().any(|off| off == source || *off == format!("{source} {host}"));
        let package_hosts = boxquell_core::os::package_hosts(&self.home, &spec.os);
        let mut groups = Vec::new();
        if !package_hosts.is_empty() {
            let name =
                boxquell_core::os::definitions(&self.home).into_iter().find(|def| def.id == spec.os).map_or_else(|| spec.os.clone(), |def| def.name);
            groups.push(("os".to_string(), format!("{name} packages"), package_hosts.iter().map(|host| (host.clone(), on("os", host))).collect()));
        }
        let Ok(catalog) = boxquell_core::recipe::Catalog::of(&self.home) else { return groups };
        for entry in &spec.recipes {
            let id = entry.split('@').next().unwrap_or(entry);
            let Some(recipe) = catalog.get(id) else { continue };
            let mut hosts: Vec<String> = Vec::new();
            for host in &recipe.allow {
                // `@packages` stands for the system's package servers, which have their own entry.
                if host != "@packages" && !hosts.contains(host) {
                    hosts.push(host.clone());
                }
            }
            if !hosts.is_empty() {
                groups.push((id.to_string(), recipe.name.clone(), hosts.iter().map(|host| (host.clone(), on(id, host))).collect()));
            }
        }
        groups
    }

    /// Switch a destination that software or the system opens, or all of a source's, on or off.
    pub fn set_auto_rule(&self, name: &str, source: &str, host: Option<&str>, enabled: bool) -> Result<Value> {
        let mut spec = self.spec(name)?;
        let groups = self.auto_rules(&spec);
        let group = groups.iter().find(|(id, _, _)| id == source).with_context(|| format!("{source} opens nothing in {name}"))?;
        match host {
            Some(host) => {
                ensure!(group.2.iter().any(|(known, _)| known == host), "{source} does not open {host}");
                let entry = format!("{source} {host}");
                spec.auto_off.retain(|off| *off != entry);
                if enabled {
                    // Switching one on again while the whole source is off: the rest stays off.
                    if spec.auto_off.iter().any(|off| off == source) {
                        spec.auto_off.retain(|off| off != source);
                        spec.auto_off.extend(group.2.iter().filter(|(other, _)| other != host).map(|(other, _)| format!("{source} {other}")));
                    }
                } else {
                    spec.auto_off.push(entry);
                }
            }
            None => {
                spec.auto_off.retain(|off| off != source && !off.starts_with(&format!("{source} ")));
                if !enabled {
                    spec.auto_off.push(source.to_string());
                }
            }
        }
        spec.save(&self.home.sandbox(name))?;
        if let Some(running) = self.running(name) {
            running.policy.write().unwrap().rules = self.effective_rules(&spec);
        }
        self.info(name)
    }

    /// The rules in force, first match wins: what is denied for all sandboxes,
    /// the sandbox's own rules, what its software and system open, and last
    /// what is allowed for all sandboxes.
    fn effective_rules(&self, spec: &SandboxSpec) -> Vec<Rule> {
        let global = self.global_rules();
        let (denies, allows): (Vec<Rule>, Vec<Rule>) = global.into_iter().partition(|rule| rule.action == boxquell_net::Action::Deny);
        let opened = self
            .auto_rules(spec)
            .into_iter()
            .flat_map(|(_, _, hosts)| hosts)
            .filter(|(_, on)| *on)
            .filter_map(|(host, _)| Rule::new(boxquell_net::Action::Allow, &host).ok());
        denies.into_iter().chain(spec.rules.iter().cloned()).chain(opened).chain(allows).collect()
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
        self.push_env(name);
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
        for mut secret in imported.secrets {
            // A key helper has to work here before the sandbox relies on it.
            if let Some(command) = &secret.helper {
                secret.value = run_helper(command).with_context(|| format!("the apiKeyHelper `{command}` did not give a key on this computer"))?;
                self.helper_runs.lock().unwrap().insert(format!("{name}/{}", secret.name), (std::time::Instant::now(), None));
            }
            self.local_secrets(name).set(secret)?;
        }
        boxquell_core::write_private(&self.claude_settings_file(name), imported.settings.as_bytes())?;
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

    /// Set or remove (empty value) environment variables for sessions in the guest, from its next start.
    pub fn set_env(&self, name: &str, changes: std::collections::BTreeMap<String, String>) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let mut spec = self.spec(name)?;
        for (key, value) in changes {
            let valid = !key.is_empty() && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && !key.as_bytes()[0].is_ascii_digit();
            ensure!(valid, "invalid variable name {key:?}: use letters, digits and '_'");
            if value.is_empty() {
                spec.env.remove(&key);
            } else {
                spec.env.insert(key, value);
            }
        }
        spec.save(&self.home.sandbox(name))?;
        self.push_env(name);
        Ok(())
    }

    /// What starts inside the guest at every boot. Entries are single lines.
    pub fn set_autostart(&self, name: &str, entries: Vec<String>) -> Result<()> {
        let mut spec = self.spec(name)?;
        let entries: Vec<String> =
            entries.iter().map(|entry| entry.trim().to_string()).filter(|entry| !entry.is_empty() && entry != "none").collect();
        ensure!(entries.iter().all(|entry| !entry.contains(['\n', '\r'])), "a start command must be a single line");
        spec.autostart = entries;
        spec.save(&self.home.sandbox(name))
    }

    /// Ask the helper commands of a sandbox's secrets for fresh values where
    /// the last ones are getting old, or for all of them with `force`. This
    /// runs programs and waits for them: call it off the async threads.
    pub fn refresh_helpers(&self, name: &str, force: bool) {
        const DEFAULT_TTL: u64 = 300;
        let store = self.local_secrets(name);
        let mut changed = false;
        for mut secret in store.list().unwrap_or_default() {
            let Some(command) = secret.helper.clone() else { continue };
            let key = format!("{name}/{}", secret.name);
            let ttl = Duration::from_secs(if secret.ttl_secs == 0 { DEFAULT_TTL } else { secret.ttl_secs });
            // Early enough that a key is never used up to its last second.
            let fresh = self.helper_runs.lock().unwrap().get(&key).is_some_and(|(at, problem)| problem.is_none() && at.elapsed() < ttl.mul_f32(0.75));
            if fresh && !force && !secret.value.is_empty() {
                continue;
            }
            let problem = match run_helper(&command) {
                Ok(value) => {
                    if value != secret.value {
                        secret.value = value;
                        changed |= store.set(secret).is_ok();
                    }
                    None
                }
                Err(err) => Some(format!("{err:#}")),
            };
            self.helper_runs.lock().unwrap().insert(key, (std::time::Instant::now(), problem));
        }
        if changed {
            let _ = self.refresh_secrets();
        }
    }

    /// Measure this computer's own load every few seconds, for the overview:
    /// how busy it is besides, and because of, the sandboxes.
    pub async fn sample_host(self: Arc<Self>) {
        // Without a way to measure on this system there is nothing to show.
        let Ok(mut system) = sysinfo::System::new() else { return };
        let home = std::fs::canonicalize(self.home.root()).unwrap_or_else(|_| self.home.root().to_path_buf());
        let mut disk = [0.0, 0.0];
        let mut round = 0u64;
        loop {
            system.refresh_cpu_usage();
            system.refresh_memory();
            // The disk that holds the sandboxes; it changes slowly, so look once a minute.
            if round.is_multiple_of(30) {
                if let Ok(disks) = sysinfo::Disks::new_with_refreshed_list() {
                    if let Some(found) = disks.iter().filter(|d| home.starts_with(d.mount_point())).max_by_key(|d| d.mount_point().as_os_str().len())
                    {
                        const GIB: f64 = (1u64 << 30) as f64;
                        disk = [(found.total_space() - found.available_space()) as f64 / GIB, found.total_space() as f64 / GIB];
                    }
                }
            }
            round += 1;
            const MIB: f64 = (1u64 << 20) as f64;
            // The first reading has nothing to compare with and says nothing.
            if round > 1 {
                let mut load = self.host_load.lock().unwrap();
                load.push_back([
                    system.global_cpu_usage() as f64,
                    system.used_memory() as f64 / MIB,
                    system.total_memory() as f64 / MIB,
                    disk[0],
                    disk[1],
                ]);
                while load.len() > SAMPLES_KEPT {
                    load.pop_front();
                }
            }
            tokio::time::sleep(Duration::from_secs(SAMPLE_EVERY)).await;
        }
    }

    /// This computer's recent load, in the shape of a sandbox's measurements.
    pub fn host_stats(&self) -> Value {
        let samples: Vec<Value> = self
            .host_load
            .lock()
            .unwrap()
            .iter()
            .map(|[cpu, used, total, disk_used, disk_total]| {
                json!({ "cpu": cpu, "mem_used_mib": used.round(), "mem_total_mib": total.round(), "disk_used_gib": disk_used, "disk_total_gib": disk_total, "rx_rate": 0, "tx_rate": 0 })
            })
            .collect();
        json!({ "cpus": std::thread::available_parallelism().map_or(1, |n| n.get()), "samples": samples })
    }

    /// Keep the keys of running sandboxes fresh for as long as the daemon lives.
    pub async fn keep_helpers(self: Arc<Self>) {
        loop {
            tokio::time::sleep(Duration::from_secs(20)).await;
            // A new day releases the keys that reached their limit yesterday.
            self.check_key_limits();
            let names: Vec<String> = self.running.lock().unwrap().keys().cloned().collect();
            for name in names {
                let daemon = self.clone();
                let _ = tokio::task::spawn_blocking(move || daemon.refresh_helpers(&name, false)).await;
            }
        }
    }

    /// Give a sandbox what its project file names beyond what creating it
    /// covers: rules, variables, start commands, limits, ports and secrets
    /// that are stored here. Nothing is taken away from the sandbox.
    pub async fn apply_project(&self, name: &str, project: &boxquell_core::project::Project) -> Result<Value> {
        let spec = self.spec(name)?;
        for rule in project.rules() {
            let rule: Rule = rule.parse().map_err(anyhow::Error::msg)?;
            if !spec.rules.contains(&rule) {
                self.add_rule(name, rule, None)?;
            }
        }
        self.set_env(name, project.env.clone())?;
        if project.workdir.is_some() {
            self.set_workdir(name, project.workdir.clone())?;
        }
        if !project.autostart.is_empty() {
            self.set_autostart(name, project.autostart.clone())?;
        }
        self.set_data_limits(name, project.upload_limit, project.download_limit)?;
        if project.idle_stop.is_some() {
            self.set_idle_stop(name, project.idle_stop)?;
        }
        for secret in &project.secrets {
            if matches!(self.secrets.get(secret), Ok(Some(_))) {
                self.attach_secret(name, secret)?;
            }
        }
        for port in &project.ports {
            let (host, guest) = port.split_once(':').unwrap_or((port, port));
            let forward: PortForward = serde_json::from_value(
                json!({ "host": host.parse::<u16>().context("invalid port")?, "guest": guest.parse::<u16>().context("invalid port")? }),
            )?;
            if !self.spec(name)?.ports.iter().any(|have| have.host == forward.host) {
                self.add_port(name, forward).await?;
            }
        }
        self.info(name)
    }

    /// Write the sandbox's description as a `boxquell.toml` into its project folder.
    pub fn write_project_file(&self, name: &str, force: bool) -> Result<PathBuf> {
        let spec = self.spec(name)?;
        let folder = spec.workspace.clone().with_context(|| format!("{name} has no project folder to put the file into"))?;
        let file = folder.join(boxquell_core::project::FILE);
        ensure!(force || !file.exists(), "{} exists already", file.display());
        std::fs::write(&file, boxquell_core::project::Project::of(&spec).to_toml()?)?;
        Ok(file)
    }

    /// Prices per million tokens, by model: the first entry whose pattern fits
    /// a model's name applies. Kept in `prices.json`, which the user can edit;
    /// without that file, the list that came with this version.
    pub fn prices(&self) -> Vec<crate::usage::Price> {
        std::fs::read(self.home.root().join("prices.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(crate::usage::default_prices)
    }

    pub fn set_prices(&self, prices: &[crate::usage::Price]) -> Result<()> {
        Ok(std::fs::write(self.home.root().join("prices.json"), serde_json::to_vec_pretty(prices)?)?)
    }

    /// Limits per key and day: the secret's name with a sum in dollars, a number of tokens, or both.
    pub fn key_limits(&self) -> std::collections::BTreeMap<String, crate::usage::Limit> {
        std::fs::read(self.home.root().join("key-limits.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn set_key_limits(&self, limits: &std::collections::BTreeMap<String, crate::usage::Limit>) -> Result<()> {
        std::fs::write(self.home.root().join("key-limits.json"), serde_json::to_vec_pretty(limits)?)?;
        self.check_key_limits();
        Ok(())
    }

    /// Token use over the last `days`, in stretches of an hour or a day, with
    /// what it costs at the price list and how each key stands against its limit.
    pub fn usage_report(&self, days: u64) -> Result<Value> {
        let hourly = days <= 2;
        let bucket = if hourly { 3_600_000 } else { 86_400_000 };
        let rows = self.store.usage(now_ms().saturating_sub(days * 86_400_000), bucket)?;
        Ok(crate::usage::report(rows, &self.prices(), &self.key_limits(), &self.today_by_key()?, bucket))
    }

    /// What each key used since midnight (UTC): tokens and dollars at the price list.
    fn today_by_key(&self) -> Result<std::collections::BTreeMap<String, (u64, f64)>> {
        let midnight = now_ms() / 86_400_000 * 86_400_000;
        let prices = self.prices();
        let mut today = std::collections::BTreeMap::new();
        for row in self.store.usage(midnight, 86_400_000)? {
            let entry: &mut (u64, f64) = today.entry(row["secret"].as_str().unwrap_or_default().to_string()).or_default();
            entry.0 += crate::usage::tokens(&row);
            entry.1 += crate::usage::cost(&row, &prices).unwrap_or(0.0);
        }
        Ok(today)
    }

    /// Pause the keys that reached their limit for the day, and release those that have not.
    pub fn check_key_limits(&self) {
        let limits = self.key_limits();
        let today = self.today_by_key().unwrap_or_default();
        let over: Vec<String> = limits
            .iter()
            .filter(|(key, limit)| {
                let (tokens, dollars) = today.get(*key).copied().unwrap_or((0, 0.0));
                limit.daily_tokens.is_some_and(|max| tokens >= max) || limit.daily_usd.is_some_and(|max| dollars >= max)
            })
            .map(|(key, _)| key.clone())
            .collect();
        let mut paused = self.paused_keys.write().unwrap();
        for key in over.iter().filter(|key| !paused.contains(key)) {
            crate::notify::show(
                &format!("The key {key} reached its limit for today"),
                "Requests with it are refused until tomorrow or until you raise the limit under Usage.",
            );
        }
        *paused = over;
    }

    /// The tasks given to a sandbox's agents, newest first.
    pub fn tasks(&self, name: &str) -> Vec<Value> {
        let mut all = records(&self.home.sandbox(name).join("tasks"));
        for task in &mut all {
            // A task that was running when the daemon ended did not finish.
            let live = self.tasks.lock().unwrap().contains_key(&format!("{name}/{}", task["id"].as_str().unwrap_or_default()));
            if task["state"] == "running" && !live {
                task["state"] = json!("interrupted");
            }
        }
        all.reverse();
        all
    }

    pub fn task_log(&self, name: &str, id: &str) -> Result<String> {
        validate_name(id)?;
        Ok(tail(&self.home.sandbox(name).join("tasks").join(format!("{id}.log")), 2000))
    }

    /// Give an installed agent a task to work through in the background, in
    /// the project root. Its output goes to a log; a notification says when
    /// it is over.
    pub async fn start_task(self: &Arc<Self>, name: &str, agent: &str, prompt: &str, snapshot: bool) -> Result<Value> {
        let prompt = prompt.trim();
        ensure!(!prompt.is_empty(), "say what the agent should do");
        let spec = self.spec(name)?;
        let catalog = boxquell_core::recipe::Catalog::of(&self.home)?;
        let recipe = catalog.find(agent)?;
        ensure!(spec.recipes.iter().any(|entry| entry.split('@').next() == Some(recipe.id.as_str())), "{} is not installed in {name}", recipe.name);
        let template = recipe
            .task
            .clone()
            .with_context(|| format!("{} cannot be given a task without a terminal; its recipe names no command for that", recipe.name))?;
        let command = template.replace("{prompt}", &shell_quote(prompt));
        // The state before the agent starts, to go back to when the result is no good.
        let mut snapshot_id = None;
        if snapshot {
            let label: String = prompt.chars().take(60).collect();
            snapshot_id = self.take_snapshot(name, &format!("before the task: {label}")).await?["id"].as_str().map(str::to_string);
        }
        let info = self.start(name).await?;
        let port = info["ssh_port"].as_u64().context("the sandbox has no ssh port")? as u16;
        let dir = self.home.sandbox(name);
        let tasks = dir.join("tasks");
        std::fs::create_dir_all(&tasks)?;
        let started = now_ms();
        let id = format!("t{}", started / 1000);
        let record = json!({ "id": id, "agent": recipe.id, "agent_name": recipe.name, "prompt": prompt, "state": "running", "started_ms": started, "created_ms": started, "snapshot": snapshot_id });
        let file = tasks.join(format!("{id}.json"));
        std::fs::write(&file, serde_json::to_vec_pretty(&record)?)?;
        let log = std::fs::File::create(tasks.join(format!("{id}.log")))?;
        let mut child = tokio::process::Command::new("ssh")
            .args(ssh::base_args(&dir.join("id_ed25519"), port))
            // A login shell loads the sandbox's environment; the agent runs where sessions start.
            .arg(format!("bash -lc {}", shell_quote(&command)))
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .kill_on_drop(true)
            .spawn()
            .context("cannot run ssh; is OpenSSH installed?")?;
        let stop = Arc::new(Notify::new());
        let key = format!("{name}/{id}");
        self.tasks.lock().unwrap().insert(key.clone(), stop.clone());
        let (daemon, sandbox, agent_name, mut ending) = (self.clone(), name.to_string(), recipe.name.clone(), record.clone());
        tokio::spawn(async move {
            let record = &mut ending;
            let (state, exit) = tokio::select! {
                status = child.wait() => match status {
                    Ok(status) if status.success() => ("done", status.code()),
                    Ok(status) => ("failed", status.code()),
                    Err(_) => ("failed", None),
                },
                _ = stop.notified() => {
                    let _ = child.kill().await;
                    ("stopped", None)
                }
            };
            daemon.tasks.lock().unwrap().remove(&key);
            record["state"] = json!(state);
            record["exit"] = json!(exit);
            record["ended_ms"] = json!(now_ms());
            let _ = std::fs::write(&file, serde_json::to_vec_pretty(record).unwrap_or_default());
            let outcome = match state {
                "done" => "finished its task",
                "stopped" => "was stopped",
                _ => "ended with an error",
            };
            crate::notify::show(&format!("{agent_name} in {sandbox} {outcome}"), record["prompt"].as_str().unwrap_or_default());
            if let Some(running) = daemon.running(&sandbox) {
                *running.busy_at.lock().unwrap() = std::time::Instant::now();
            }
        });
        Ok(record)
    }

    /// End a running task.
    pub fn stop_task(&self, name: &str, id: &str) -> Result<()> {
        let stop = self.tasks.lock().unwrap().get(&format!("{name}/{id}")).cloned().with_context(|| format!("the task {id} is not running"))?;
        stop.notify_one();
        Ok(())
    }

    /// Take a task that is over off the list, with its log.
    pub fn delete_task(&self, name: &str, id: &str) -> Result<()> {
        validate_name(id)?;
        ensure!(!self.tasks.lock().unwrap().contains_key(&format!("{name}/{id}")), "the task is still running; stop it first");
        for extension in ["json", "log"] {
            let _ = std::fs::remove_file(self.home.sandbox(name).join("tasks").join(format!("{id}.{extension}")));
        }
        Ok(())
    }

    /// The project folder, the sandbox's copy of it and the base of a sandbox in review mode.
    fn review_parts(&self, name: &str) -> Result<(PathBuf, PathBuf, crate::review::Base)> {
        let spec = self.spec(name)?;
        ensure!(spec.review, "{name} is not in review mode");
        let project = spec.workspace.context("the sandbox has no project folder")?;
        let dir = self.home.sandbox(name);
        let base = std::fs::read(dir.join("review-base.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default();
        Ok((project, dir.join("work"), base))
    }

    /// Switch review mode on or off. On: a copy of the project folder is made
    /// and the sandbox works on that from its next start. Off: the sandbox
    /// works in the project folder again; the copy is removed. A running
    /// sandbox is restarted, because what it sees as /workspace changes.
    pub async fn set_review(self: &Arc<Self>, name: &str, on: bool) -> Result<Value> {
        let mut spec = self.spec(name)?;
        if spec.review == on {
            return self.info(name);
        }
        let project =
            spec.workspace.clone().with_context(|| format!("{name} has no project folder; review mode is for sandboxes that work on one"))?;
        let dir = self.home.sandbox(name);
        if !on {
            let (project, work, base) = self.review_parts(name)?;
            let open = tokio::task::spawn_blocking(move || crate::review::changes(&project, &work, &base)).await?;
            let pending = open.iter().filter(|change| change["side"] != "project").count();
            ensure!(pending == 0, "{pending} changes of the sandbox are not taken over or discarded yet; decide about them first");
        }
        let was_running = self.running(name).is_some();
        self.stop(name).await?;
        if on {
            let work = dir.join("work");
            let base = tokio::task::spawn_blocking(move || crate::review::start(&project, &work)).await??;
            std::fs::write(dir.join("review-base.json"), serde_json::to_vec(&base)?)?;
        } else {
            let _ = std::fs::remove_dir_all(dir.join("work"));
            let _ = std::fs::remove_file(dir.join("review-base.json"));
        }
        spec.review = on;
        spec.save(&dir)?;
        if was_running {
            self.start(name).await?;
        }
        self.info(name)
    }

    /// What differs between the project and the sandbox's copy.
    pub fn review_changes(&self, name: &str) -> Result<Value> {
        let (project, work, base) = self.review_parts(name)?;
        Ok(json!({ "project": project, "changes": crate::review::changes(&project, &work, &base) }))
    }

    pub fn review_diff(&self, name: &str, path: &str, towards_project: bool) -> Result<Value> {
        let (project, work, _) = self.review_parts(name)?;
        crate::review::diff(&project, &work, path, towards_project)
    }

    /// Take the sandbox's state of these files into the project, or the
    /// project's state into the sandbox's copy. Without paths: every file the
    /// sandbox changed (for taking over or discarding) or that changed in the
    /// project (for bringing in), but never one both sides changed.
    pub fn review_settle(&self, name: &str, paths: Option<Vec<String>>, to_project: bool, from_project_only: bool) -> Result<Value> {
        let _guard = self.handled_lock.lock().unwrap();
        let (project, work, mut base) = self.review_parts(name)?;
        let paths = match paths {
            Some(paths) => paths,
            None => crate::review::changes(&project, &work, &base)
                .iter()
                .filter(|change| change["side"] == if from_project_only { "project" } else { "sandbox" })
                .filter_map(|change| change["path"].as_str().map(str::to_string))
                .collect(),
        };
        let done = crate::review::settle(&project, &work, &mut base, &paths, to_project)?;
        std::fs::write(self.home.sandbox(name).join("review-base.json"), serde_json::to_vec(&base)?)?;
        Ok(json!({ "done": done, "project": project, "changes": crate::review::changes(&project, &work, &base) }))
    }

    /// Run an action of an installed recipe in the sandbox and return what it
    /// printed. Actions that need a terminal are opened by the interface instead.
    pub async fn run_action(
        self: &Arc<Self>,
        name: &str,
        recipe: &str,
        index: usize,
        values: &std::collections::BTreeMap<String, String>,
    ) -> Result<Value> {
        let spec = self.spec(name)?;
        let catalog = boxquell_core::recipe::Catalog::of(&self.home)?;
        let recipe = catalog.find(recipe)?;
        ensure!(spec.recipes.iter().any(|entry| entry.split('@').next() == Some(recipe.id.as_str())), "{} is not installed in {name}", recipe.name);
        let action = recipe.actions.get(index).with_context(|| format!("{} has no such action", recipe.name))?;
        let command = action.command_with(values)?;
        let info = self.start(name).await?;
        let port = info["ssh_port"].as_u64().context("the sandbox has no ssh port")? as u16;
        let run = tokio::process::Command::new("ssh")
            .args(ssh::base_args(&self.home.sandbox(name).join("id_ed25519"), port))
            .arg(format!("bash -lc {}", shell_quote(&format!("{command} 2>&1"))))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .output();
        let output = tokio::time::timeout(Duration::from_secs(180), run)
            .await
            .context("it did not finish within three minutes; a command that asks questions needs a terminal")??;
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok(json!({ "ok": output.status.success(), "output": text.trim_end() }))
    }

    fn templates_dir(&self) -> PathBuf {
        self.home.root().join("templates")
    }

    /// The saved states of a sandbox's disk, oldest first.
    pub fn snapshots(&self, name: &str) -> Vec<Value> {
        records(&self.home.sandbox(name).join("snapshots"))
    }

    /// Sandboxes saved as a starting point for new ones.
    pub fn templates(&self) -> Vec<Value> {
        records(&self.templates_dir())
    }

    /// Stop the sandbox when it runs, do something with its disk, and start it
    /// again when it ran. A disk is only copied while nothing writes to it.
    async fn with_disk_at_rest<T: Send + 'static>(
        self: &Arc<Self>,
        name: &str,
        work: impl FnOnce(PathBuf) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        self.spec(name)?;
        let was_running = self.running(name).is_some();
        self.stop(name).await?;
        let dir = self.home.sandbox(name);
        let outcome = {
            let _guard = self.lifecycle.lock().await;
            tokio::task::spawn_blocking(move || work(dir)).await?
        };
        if was_running {
            self.start(name).await?;
        }
        outcome
    }

    /// Save the state of a sandbox's disk, to come back to later.
    pub async fn take_snapshot(self: &Arc<Self>, name: &str, label: &str) -> Result<Value> {
        let spec = self.spec(name)?;
        let label = label.trim().to_string();
        let record = self
            .with_disk_at_rest(name, move |dir| {
                let disk = dir.join("disk.img");
                ensure!(disk.exists(), "the sandbox has no disk yet: start it once first");
                let created = now_ms();
                let id = format!("s{}", created / 1000);
                let snapshots = dir.join("snapshots");
                std::fs::create_dir_all(&snapshots)?;
                copy_disk(&disk, &snapshots.join(format!("{id}.img"))).context("cannot copy the disk")?;
                let record =
                    json!({ "id": id, "label": label, "created_ms": created, "recipes": spec.recipes, "recipe_revisions": spec.recipe_revisions });
                std::fs::write(snapshots.join(format!("{id}.json")), serde_json::to_vec_pretty(&record)?)?;
                Ok(record)
            })
            .await?;
        Ok(record)
    }

    /// Put a sandbox's disk back to a saved state. What happened in the
    /// sandbox since is gone; the snapshot itself stays.
    pub async fn restore_snapshot(self: &Arc<Self>, name: &str, id: &str) -> Result<Value> {
        validate_name(id)?;
        let record = self.snapshots(name).into_iter().find(|record| record["id"] == id).with_context(|| format!("{name} has no snapshot {id}"))?;
        let image = format!("{id}.img");
        self.with_disk_at_rest(name, move |dir| {
            let tmp = dir.join("disk.img.partial");
            copy_disk(&dir.join("snapshots").join(image), &tmp).context("cannot copy the snapshot")?;
            Ok(std::fs::rename(&tmp, dir.join("disk.img"))?)
        })
        .await?;
        // The software list goes back with the disk.
        let mut spec = self.spec(name)?;
        spec.recipes = serde_json::from_value(record["recipes"].clone()).unwrap_or_default();
        spec.recipe_revisions = serde_json::from_value(record["recipe_revisions"].clone()).unwrap_or_default();
        spec.save(&self.home.sandbox(name))?;
        self.info(name)
    }

    pub fn delete_snapshot(&self, name: &str, id: &str) -> Result<Value> {
        validate_name(id)?;
        let dir = self.home.sandbox(name).join("snapshots");
        ensure!(dir.join(format!("{id}.json")).exists(), "{name} has no snapshot {id}");
        for extension in ["img", "json"] {
            let _ = std::fs::remove_file(dir.join(format!("{id}.{extension}")));
        }
        self.info(name)
    }

    /// Save a sandbox as a template: its disk as it is now, with its system,
    /// software and variables. New sandboxes can start from it.
    pub async fn save_template(self: &Arc<Self>, name: &str, template: &str, description: &str) -> Result<Value> {
        validate_name(template)?;
        let spec = self.spec(name)?;
        let templates = self.templates_dir();
        let (template, description) = (template.to_string(), description.trim().to_string());
        self.with_disk_at_rest(name, move |dir| {
            let disk = dir.join("disk.img");
            ensure!(disk.exists(), "the sandbox has no disk yet: start it once first");
            std::fs::create_dir_all(&templates)?;
            copy_disk(&disk, &templates.join(format!("{template}.img"))).context("cannot copy the disk")?;
            let record = json!({
                "name": template, "description": description, "created_ms": now_ms(), "from": spec.name,
                "os": spec.os, "recipes": spec.recipes, "recipe_revisions": spec.recipe_revisions, "env": spec.env, "disk_gib": spec.disk_gib,
            });
            std::fs::write(templates.join(format!("{template}.json")), serde_json::to_vec_pretty(&record)?)?;
            Ok(record)
        })
        .await
    }

    pub fn delete_template(&self, template: &str) -> Result<()> {
        validate_name(template)?;
        let dir = self.templates_dir();
        ensure!(dir.join(format!("{template}.json")).exists(), "there is no template {template:?}");
        for extension in ["img", "json"] {
            let _ = std::fs::remove_file(dir.join(format!("{template}.{extension}")));
        }
        Ok(())
    }

    /// Blocked destinations the user has dealt with, as `<sandbox> <destination>`
    /// with the time up to which they count as seen. Kept in a file, so they
    /// do not come back when the interface is opened again.
    pub fn handled_notifications(&self) -> HashMap<String, u64> {
        std::fs::read(self.home.root().join("notifications.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    pub fn mark_handled(&self, items: Vec<(String, u64)>) -> Result<HashMap<String, u64>> {
        let _guard = self.handled_lock.lock().unwrap();
        let mut handled = self.handled_notifications();
        for (key, until) in items {
            let seen = handled.entry(key).or_insert(0);
            *seen = (*seen).max(until);
        }
        // Destinations nobody has tried for a month need no entry any more.
        let month_ago = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64)
            .saturating_sub(30 * 24 * 3600 * 1000);
        handled.retain(|_, until| *until >= month_ago);
        std::fs::write(self.home.root().join("notifications.json"), serde_json::to_vec(&handled)?)?;
        Ok(handled)
    }

    /// Give a running sandbox its variables anew, after a variable or a
    /// secret's placeholder changed; sessions opened from now on see them.
    fn push_env(&self, name: &str) {
        let (Some(running), Ok(spec)) = (self.running(name), self.spec(name)) else { return };
        let env = guest_env(&spec, &self.sandbox_secrets(&spec).unwrap_or_default());
        let key = self.home.sandbox(name).join("id_ed25519");
        tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            let child = tokio::process::Command::new("ssh")
                .args(ssh::base_args(&key, running.ssh_port))
                .arg("sudo tee /etc/profile.d/boxquell.sh >/dev/null")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn();
            if let Ok(mut child) = child {
                if let Some(mut stdin) = child.stdin.take() {
                    let _ = stdin.write_all(env.as_bytes()).await;
                }
                let _ = tokio::time::timeout(Duration::from_secs(20), child.wait()).await;
            }
        });
    }

    /// The SSH keys every sandbox may sign with.
    pub fn global_ssh_keys(&self) -> SshKeys {
        std::fs::read(self.home.root().join("ssh-keys.json")).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
    }

    /// The choice for one sandbox, or for all with `None`, with what there is to choose from.
    pub fn ssh_keys(&self, name: Option<&str>) -> Result<Value> {
        let global = self.global_ssh_keys();
        let own = match name {
            Some(name) => self.spec(name)?.ssh,
            None => global.clone(),
        };
        Ok(json!({
            "agent": own.agent,
            "files": own.files,
            "global": global,
            "problems": crate::keyagent::problems(&if name.is_some() { global.with(&own) } else { own.clone() }),
            "available": { "agent": crate::keyagent::host_agent().is_some(), "keys": crate::keyagent::host_keys(), "supported": cfg!(unix) },
        }))
    }

    /// Choose the SSH keys for one sandbox, or for all with `None`. Running sandboxes follow at once.
    pub fn set_ssh_keys(&self, name: Option<&str>, mut keys: SshKeys) -> Result<Value> {
        keys.files.dedup();
        for file in &keys.files {
            ensure!(file.is_absolute(), "{} is not a full path", file.display());
            ensure!(crate::keyagent::needs_passphrase(file).is_some(), "{} is not a private SSH key", file.display());
        }
        match name {
            Some(name) => {
                let mut spec = self.spec(name)?;
                spec.ssh = keys;
                spec.save(&self.home.sandbox(name))?;
            }
            None => std::fs::write(self.home.root().join("ssh-keys.json"), serde_json::to_vec_pretty(&keys)?)?,
        }
        let running: Vec<(String, Arc<Running>)> =
            self.running.lock().unwrap().iter().map(|(name, running)| (name.clone(), running.clone())).collect();
        for (sandbox, running) in running {
            if name.is_none_or(|name| name == sandbox) {
                if let Ok(spec) = self.spec(&sandbox) {
                    self.connect_ssh_keys(&sandbox, &running, &spec);
                }
            }
        }
        self.ssh_keys(name)
    }

    /// (Re)connect a running sandbox's SSH agent socket to the keys chosen for it.
    fn connect_ssh_keys(&self, name: &str, running: &Running, spec: &SandboxSpec) {
        let dir = self.home.sandbox(name);
        let keys = self.global_ssh_keys().with(&spec.ssh);
        let task = tokio::spawn(crate::keyagent::serve(dir.join("id_ed25519"), running.ssh_port, dir, keys));
        if let Some(old) = running.key_agent.lock().unwrap().replace(task) {
            old.abort();
        }
    }

    /// Where shells and agents start in the guest; `None` goes back to `/workspace`.
    pub fn set_workdir(&self, name: &str, workdir: Option<String>) -> Result<Value> {
        let mut spec = self.spec(name)?;
        if let Some(path) = &workdir {
            boxquell_core::spec::validate_guest_path(path)?;
        }
        spec.workdir = workdir;
        spec.save(&self.home.sandbox(name))?;
        self.info(name)
    }

    /// Share another directory of this computer with the sandbox, from its next start.
    pub fn add_share(&self, name: &str, mut share: boxquell_core::Share) -> Result<Value> {
        let mut spec = self.spec(name)?;
        ensure!(spec.backend == Backend::Krun, "the qemu backend cannot share directories yet");
        boxquell_core::spec::validate_guest_path(&share.guest)?;
        ensure!(share.guest != "/workspace", "/workspace is the project folder of the sandbox");
        ensure!(!spec.shares.iter().any(|s| s.guest == share.guest), "{} is already shared into {name}", share.guest);
        share.host = std::fs::canonicalize(&share.host).with_context(|| format!("{} not found", share.host.display()))?;
        ensure!(share.host.is_dir(), "{} is not a directory", share.host.display());
        spec.shares.push(share);
        spec.save(&self.home.sandbox(name))?;
        self.info(name)
    }

    pub fn remove_share(&self, name: &str, guest: &str) -> Result<Value> {
        let mut spec = self.spec(name)?;
        spec.shares.retain(|s| s.guest != guest);
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
        if let Ok(catalog) = boxquell_core::recipe::Catalog::of(&self.home) {
            for id in ids {
                let id = id.split('@').next().unwrap_or(id);
                if let Some(recipe) = catalog.get(id) {
                    spec.recipe_revisions.insert(id.to_string(), recipe.revision.clone());
                }
            }
        }
        spec.save(&self.home.sandbox(name))?;
        // What the new software opens applies from now on.
        if let Some(running) = self.running(name) {
            running.policy.write().unwrap().rules = self.effective_rules(&spec);
        }
        self.info(name)
    }

    /// Take recipes off the list of what is installed, after they were removed from the guest.
    pub fn forget_recipes(&self, name: &str, ids: &[String]) -> Result<Value> {
        let mut spec = self.spec(name)?;
        for id in ids {
            let id = id.split('@').next().unwrap_or(id);
            spec.recipes.retain(|old| old.split('@').next() != Some(id));
            spec.recipe_revisions.remove(id);
            spec.auto_off.retain(|off| off != id && !off.starts_with(&format!("{id} ")));
        }
        spec.save(&self.home.sandbox(name))?;
        if let Some(running) = self.running(name) {
            running.policy.write().unwrap().rules = self.effective_rules(&spec);
        }
        self.info(name)
    }

    /// Installed software for which there is something newer: a newer version
    /// on offer, or a recipe that changed since the install.
    fn recipe_updates(&self, spec: &SandboxSpec) -> Vec<Value> {
        let Ok(catalog) = boxquell_core::recipe::Catalog::of(&self.home) else { return Vec::new() };
        let mut updates = Vec::new();
        for entry in &spec.recipes {
            let (id, version) = entry.split_once('@').map_or((entry.as_str(), None), |(id, version)| (id, Some(version)));
            let Some(recipe) = catalog.get(id) else { continue };
            let newest = recipe.versions.iter().max_by(|a, b| boxquell_core::recipe::compare_versions(a, b));
            if let (Some(have), Some(newest)) = (version, newest) {
                if boxquell_core::recipe::compare_versions(newest, have) == std::cmp::Ordering::Greater {
                    updates.push(
                        json!({ "id": id, "kind": "version", "to": format!("{id}@{newest}"), "note": format!("version {newest} is on offer") }),
                    );
                    continue;
                }
            }
            if spec.recipe_revisions.get(id).is_some_and(|installed| *installed != recipe.revision) {
                updates.push(json!({ "id": id, "kind": "recipe", "to": entry, "note": "the recipe changed since it was installed" }));
            }
        }
        updates
    }

    /// Install recipes in the background by running `boxquell add`, or remove
    /// them with `boxquell remove`; progress is in `info().job`.
    pub fn start_install(self: &Arc<Self>, name: &str, ids: Vec<String>, verb: &str, again: bool) -> Result<Value> {
        self.spec(name)?;
        ensure!(!ids.is_empty(), "choose at least one recipe");
        boxquell_core::recipe::Catalog::of(&self.home)?.resolve(&ids)?;
        {
            let mut jobs = self.jobs.lock().unwrap();
            ensure!(jobs.get(name).is_none_or(|job| job.state != "running"), "software is already being installed or removed in {name}");
            jobs.insert(name.to_string(), Job { state: "running", recipes: ids.clone(), log: String::new() });
        }
        let mut child = tokio::process::Command::new(EXE.get().context("daemon executable path is unknown")?)
            .arg(verb)
            .args(again.then_some("--again"))
            // What follows is taken as names, whatever it starts with.
            .arg("--")
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
        self.push_env(name);
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
        let base = boxquell_core::os::image(&self.home, &spec.os);

        let disk = dir.join("disk.img");
        if !disk.exists() {
            // A sandbox made from a template starts with the template's disk.
            let template = std::fs::read_to_string(dir.join("template")).ok().map(|name| self.templates_dir().join(format!("{}.img", name.trim())));
            let base = match template {
                Some(template) => {
                    ensure!(template.exists(), "the template this sandbox was made from is gone ({})", template.display());
                    template
                }
                None => base.clone(),
            };
            ensure!(base.exists(), "the system {:?} is not on this computer yet; run `boxquell os install {}`", spec.os, spec.os);
            let (from, to, tmp) = (base.clone(), disk.clone(), dir.join("disk.img.partial"));
            tokio::task::spawn_blocking(move || copy_disk(&from, &tmp).and_then(|()| std::fs::rename(&tmp, &to)))
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

        // Keys that come from a helper command are fetched before the guest can ask for them.
        let (daemon, sandbox) = (self.clone(), name.to_string());
        let _ = tokio::task::spawn_blocking(move || daemon.refresh_helpers(&sandbox, true)).await;
        // The guest sees placeholders only; the gateway holds the real values.
        let secrets = self.sandbox_secrets(&spec)?;
        let env = guest_env(&spec, &secrets);
        // Shared directories: the VM gets them by tag, the guest learns from `mounts` where they go.
        let shares: Vec<(String, PathBuf)> = spec.shares.iter().enumerate().map(|(i, share)| (format!("share{i}"), share.host.clone())).collect();
        let mounts: String = spec
            .shares
            .iter()
            .enumerate()
            .map(|(i, share)| format!("share{i} {} {}\n", share.guest, if share.read_only { "ro" } else { "rw" }))
            .collect();
        let claude_settings = std::fs::read(self.claude_settings_file(name)).ok();
        // What starts with the sandbox: an agent's name stands for its recipe's start command.
        let autostart: String = {
            let catalog = boxquell_core::recipe::Catalog::of(&self.home).ok();
            let command_of = |entry: &str| {
                let catalog = catalog.as_ref()?;
                spec.recipes.iter().find_map(|installed| {
                    let id = installed.split('@').next()?;
                    (id.rsplit(':').next()? == entry).then(|| catalog.get(id)?.command.clone()).flatten()
                })
            };
            spec.autostart.iter().map(|entry| format!("{}\n", command_of(entry).unwrap_or_else(|| entry.clone()))).collect()
        };
        // Instructions, skills and MCP servers for the agents: the global set with this sandbox's on top.
        let agent_files = boxquell_core::agentcfg::guest_files(
            &boxquell_core::agentcfg::dir(&self.home, None),
            &boxquell_core::agentcfg::dir(&self.home, Some(name)),
        );
        let agent_paths: Vec<String> = agent_files.iter().map(|(path, _, _)| format!("agent/{path}")).collect();
        let mut drive_files = vec![
            DriveFile { path: "ca.crt", contents: self.ca.cert_pem().as_bytes(), mode: 0o644 },
            DriveFile { path: "authorized_keys", contents: &public_key, mode: 0o600 },
            DriveFile { path: "env.sh", contents: env.as_bytes(), mode: 0o644 },
        ];
        for ((_, contents, executable), path) in agent_files.iter().zip(&agent_paths) {
            drive_files.push(DriveFile { path, contents, mode: if *executable { 0o755 } else { 0o644 } });
        }
        if !autostart.is_empty() {
            drive_files.push(DriveFile { path: "autostart", contents: autostart.as_bytes(), mode: 0o644 });
        }
        if !mounts.is_empty() {
            drive_files.push(DriveFile { path: "mounts", contents: mounts.as_bytes(), mode: 0o644 });
        }
        if let Some(settings) = &claude_settings {
            drive_files.push(DriveFile { path: "claude-managed-settings.json", contents: settings, mode: 0o644 });
        }
        let config_drive = dir.join("config.tar");
        drive::write(&config_drive, &drive_files)?;

        let (listener, net) = LinkListener::bind(&dir).await?;
        // In review mode the sandbox gets its copy of the project folder, not the folder itself.
        let workspace = if spec.review { Some(dir.join("work")).filter(|work| work.is_dir()) } else { spec.workspace.clone() };
        ensure!(!spec.review || workspace.is_some(), "the copy of the project folder is gone; switch review mode off and on again");
        let vm = VmSpec { cpus: spec.cpus, ram_mib: spec.ram_mib, disk, config_drive, net, workspace, shares };
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
        // Two VMs writing to one disk would destroy it: end a VM that an earlier daemon left behind.
        let pid_file = dir.join("vm.pid");
        if let Some(old) = read_pid(&pid_file) {
            end_leftover(old, &dir.join("disk.img").to_string_lossy());
        }
        let mut child = command.spawn().context("cannot start the VM process")?;
        if let Some(pid) = child.id() {
            let _ = std::fs::write(&pid_file, pid.to_string());
        }

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
        let intercept = Intercept {
            ca: self.ca.clone(),
            secrets: live_secrets.clone(),
            log_all_requests: false,
            roots: self.roots.clone(),
            paused: self.paused_keys.clone(),
        };
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
                // The tokens a request used go to the usage records, not into the connection log.
                if event.usage.is_some() {
                    if let Err(err) = daemon.store.record_usage(&sandbox, &event) {
                        eprintln!("boxquell: cannot store usage: {err:#}");
                    }
                    daemon.check_key_limits();
                    continue;
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
            key_agent: Mutex::default(),
            busy_at: Mutex::new(std::time::Instant::now()),
            cut: false.into(),
            traffic_base: Mutex::default(),
            traffic_noted: Mutex::default(),
            allowed: 0.into(),
            blocked: 0.into(),
            pid: child.id(),
            paused: false.into(),
            stop: stop.clone(),
            exited,
        });
        running.gateway.set_speed(spec.upload_kbit.unwrap_or(0) * 125, spec.download_kbit.unwrap_or(0) * 125);
        self.running.lock().unwrap().insert(name.to_string(), running.clone());
        let sampler = tokio::spawn(sample_load(running.clone(), dir.join("id_ed25519")));
        self.connect_ssh_keys(name, &running, &spec);
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
            if let Some(task) = ending.key_agent.lock().unwrap().take() {
                task.abort();
            }
            for (_, task) in ending.forwards.lock().unwrap().drain() {
                task.abort();
            }
            daemon.running.lock().unwrap().remove(&sandbox);
            let _ = std::fs::remove_file(daemon.home.sandbox(&sandbox).join("vm.pid"));
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
    let address = port.bind.unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
    let listener = TcpListener::bind((address, port.host)).await.with_context(|| format!("cannot listen on {address}:{}", port.host))?;
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

use boxquell_core::os::GUEST_SCRIPTS;

/// Bring the guest's start and stop scripts on a stopped sandbox disk up to
/// this version, so sandboxes created by an older version keep booting.
/// Uses `debugfs` from e2fsprogs, which edits an ext4 image without root.
/// Without it nothing is changed and the disk boots with what it has.
fn sync_guest_scripts(disk: &Path) {
    let debugfs = |args: &[&std::ffi::OsStr]| {
        std::process::Command::new(boxquell_core::os::e2fs_tool("debugfs")).args(args).arg(disk).stdin(Stdio::null()).stderr(Stdio::null()).output()
    };
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
        // A disk whose last run did not end cleanly still has changes waiting in
        // its journal. Writing next to them would damage the filesystem, so the
        // journal is applied first; without the checker the disk is left alone.
        let checked = std::process::Command::new(boxquell_core::os::e2fs_tool("e2fsck"))
            .arg("-p")
            .arg(disk)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        // 0: clean, 1: problems corrected. Anything else needs a person to look.
        let safe = checked.is_ok_and(|status| matches!(status.code(), Some(0 | 1)));
        let list = dir.join("commands");
        if safe && std::fs::write(&list, commands).is_ok() {
            let _ = debugfs(&["-w".as_ref(), "-f".as_ref(), list.as_os_str()]);
        } else if !safe {
            eprintln!("boxquell: {} was not updated: its filesystem could not be checked (is e2fsck installed?)", disk.display());
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

/// Where the `boxquell` program is, for those who start it themselves.
pub fn program() -> Option<PathBuf> {
    EXE.get().cloned()
}

pub async fn serve(home: Home) -> Result<()> {
    EXE.get_or_init(|| std::env::current_exe().unwrap_or_else(|_| PathBuf::from("boxquell")));
    // A daemon that stopped answering must not keep its sandboxes running next to ours.
    if let Some(old) = std::fs::read(home.daemon_file()).ok().and_then(|bytes| serde_json::from_slice::<DaemonFile>(&bytes).ok()) {
        if old.pid != std::process::id() {
            end_leftover(old.pid, " daemon");
        }
    }
    let daemon = Daemon::new(home)?;
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let file = DaemonFile { port: listener.local_addr()?.port(), token: daemon.token.clone(), pid: std::process::id() };
    let path: PathBuf = daemon.home.daemon_file();
    boxquell_core::write_private(&path, &serde_json::to_vec(&file)?)?;
    eprintln!("boxquell daemon listening on 127.0.0.1:{}", file.port);

    let app = crate::api::router(daemon.clone());
    // Extra addresses the user chose in the settings. They end with the process.
    let settings = daemon.settings();
    for address in settings.listen {
        match TcpListener::bind((address, settings.listen_port)).await {
            Ok(extra) => {
                eprintln!("boxquell daemon also listening on {address}:{} (not encrypted)", settings.listen_port);
                let app = app.clone();
                tokio::spawn(async move {
                    let _ = axum::serve(extra, app).await;
                });
            }
            Err(err) => eprintln!("boxquell: cannot listen on {address}:{}: {err}", settings.listen_port),
        }
    }
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
    // The API keeps answering while the sandboxes shut down. It is not asked to
    // finish its connections first: a browser tab holds event streams and
    // terminals open for as long as it lives, and waiting for them would leave
    // the sandboxes running under a daemon nobody can reach.
    let server = tokio::spawn(async move { axum::serve(listener, app).await });
    let upkeep = tokio::spawn(daemon.clone().keep_current());
    let helpers = tokio::spawn(daemon.clone().keep_helpers());
    let host = tokio::spawn(daemon.clone().sample_host());
    let rules = tokio::spawn(daemon.clone().keep_rules());
    shutdown.await;
    upkeep.abort();
    helpers.abort();
    host.abort();
    rules.abort();
    daemon.stop_all().await;
    let _ = std::fs::remove_file(&path);
    server.abort();
    Ok(())
}

/// End a process left over from an earlier daemon, if `pid` still is one:
/// its command line has to contain `marker`, so a reused process id is never hit.
fn end_leftover(pid: u32, marker: &str) {
    #[cfg(unix)]
    {
        let command = std::process::Command::new("ps").args(["-p", &pid.to_string(), "-o", "command="]).output();
        let is_ours = command.is_ok_and(|output| {
            let line = String::from_utf8_lossy(&output.stdout).to_string();
            // A VM is recognised by its disk; a daemon needs the program name as well.
            line.contains(marker) && (marker != " daemon" || line.contains("boxquell"))
        });
        if is_ours {
            eprintln!("boxquell: ending leftover process {pid} ({marker})");
            let _ = std::process::Command::new("kill").args(["-KILL", &pid.to_string()]).status();
            std::thread::sleep(Duration::from_millis(300));
        }
    }
    #[cfg(not(unix))]
    let _ = (pid, marker);
}

fn read_pid(file: &Path) -> Option<u32> {
    std::fs::read_to_string(file).ok()?.trim().parse().ok()
}
