// Client for the daemon API. The token arrives in the URL fragment (`boxquell ui`)
// and is kept for this browser tab only.

const fromHash = new URLSearchParams(location.hash.slice(1)).get("token");
if (fromHash) {
  sessionStorage.setItem("boxquell-token", fromHash);
  history.replaceState(null, "", location.pathname + location.search);
}
export const token = sessionStorage.getItem("boxquell-token") ?? "";

export interface Job {
  state: "running" | "done" | "failed";
  recipes: string[];
  log: string;
}

export interface Sandbox {
  name: string;
  running: boolean;
  paused: boolean;
  ssh_port: number | null;
  workspace: string | null;
  cpus: number;
  ram_mib: number;
  disk_gib: number;
  backend: "krun" | "qemu";
  /** Operating system id, e.g. debian */
  os?: string;
  /** The system's name as its definition gives it, e.g. "Debian 13". */
  os_name?: string | null;
  os_icon?: string | null;
  os_icon_color?: string | null;
  family?: string;
  mode: "whitelist" | "blacklist";
  rules: string[];
  secrets: string[];
  recipes: string[];
  local_secrets: Secret[];
  claude_settings: boolean;
  ports: { host: number; guest: number; bind?: string }[];
  shares: { host: string; guest: string; read_only: boolean }[];
  /** Agents or commands started with the sandbox */
  autostart?: string[];
  /** Where shells and agents start; null means /workspace */
  workdir: string | null;
  /** Addresses as seen from inside the sandbox: its own, this computer's, and the name server. */
  addresses?: { sandbox: string; host: string; dns: string };
  /** What keeps installed agents from working, e.g. a missing credential. */
  warnings: string[];
  /** Present in the answer to changing the Claude settings. */
  notes?: string[];
  /** Minutes without work after which it is stopped; 0 never, absent: as set for all sandboxes */
  idle_stop_minutes?: number | null;
  /** Speed limits in kbit per second */
  upload_kbit?: number | null;
  download_kbit?: number | null;
  /** Rules that end by themselves: the rule with the time it ends, in seconds since 1970 */
  rule_expiry?: Record<string, number>;
  /** Bytes to and from destinations outside since the start, against the limits */
  traffic?: { up: number; down: number; upload_limit_mib: number | null; download_limit_mib: number | null; cut: boolean };
  /** Review mode: it works on a copy of the project folder */
  review?: boolean;
  /** Tasks given to its agents, newest first */
  tasks?: Task[];
  /** Saved states of the disk, oldest first */
  snapshots?: Snapshot[];
  /** Installed software for which there is something newer */
  recipe_updates?: { id: string; kind: "version" | "recipe"; to: string; note: string }[];
  /** What the system and installed software open by themselves, each destination with whether it is on */
  auto_rules?: { source: string; name: string; hosts: { host: string; enabled: boolean }[] }[];
  /** Environment variables for sessions in the sandbox */
  env?: Record<string, string>;
  job: Job | null;
}

/** One setting of a piece of software, kept as an environment variable of the sandbox. */
export interface ConfigOption {
  env: string;
  label: string;
  help?: string;
  /** `text` (default), `number`, `switch` or `choice` */
  kind?: string;
  options?: string[];
  placeholder?: string;
  /** For a switch: the value that means on */
  on?: string;
}

/** A project's sandbox as its `boxquell.toml` describes it. */
export interface ProjectFile {
  name?: string;
  os?: string;
  software?: string[];
  cpus?: number;
  memory?: number;
  disk?: number;
  mode?: string;
  allow?: string[];
  read?: string[];
  deny?: string[];
  ports?: string[];
  secrets?: string[];
  env?: Record<string, string>;
  review?: boolean;
}

/** Tokens used in one stretch of time by one sandbox with one key and model. */
export interface UsageRow {
  t: number;
  sandbox: string;
  secret: string;
  model: string;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  requests: number;
  /** Dollars at the price list; `null` when no price fits the model */
  cost: number | null;
}

export interface UsageSum {
  name: string;
  tokens: number;
  cost: number;
  requests: number;
}

export interface UsageReport {
  bucket_ms: number;
  rows: UsageRow[];
  by_key: UsageSum[];
  by_sandbox: UsageSum[];
  by_model: UsageSum[];
  limits: { key: string; daily_usd: number | null; daily_tokens: number | null; tokens_today: number; cost_today: number; reached: boolean }[];
  unpriced: string[];
}

/** Prices of a model or family, in US dollars per million tokens. */
export interface Price {
  model: string;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
}

/** A file that differs between the project and the sandbox's copy of it. */
export interface Change {
  path: string;
  kind: "added" | "deleted" | "modified";
  /** Who changed it since the copy was made */
  side: "sandbox" | "project" | "both";
  binary: boolean;
  large: boolean;
  added: number;
  removed: number;
}

/** One line of a file's difference. */
export interface DiffRow {
  kind: "ctx" | "add" | "del" | "gap";
  old?: number | null;
  new?: number | null;
  text?: string;
}

/** A task given to an agent to work through in the background. */
export interface Task {
  id: string;
  agent: string;
  agent_name: string;
  prompt: string;
  state: "running" | "done" | "failed" | "stopped" | "interrupted";
  started_ms: number;
  ended_ms?: number;
  exit?: number | null;
  /** The snapshot taken before it started */
  snapshot?: string | null;
}

/** A saved state of a sandbox's disk. */
export interface Snapshot {
  id: string;
  label: string;
  created_ms: number;
  recipes: string[];
}

/** A sandbox saved as a starting point for new ones. */
export interface Template {
  name: string;
  description: string;
  created_ms: number;
  from: string;
  os: string;
  recipes: string[];
}

/** A secret a recipe can use, described so that it can be stored from the Config tab. */
export interface Credential {
  name: string;
  label?: string;
  help?: string;
  url?: string;
  hosts?: string[];
  env?: string[];
}

export interface Recipe {
  /** What the secrets named in `secrets` are */
  credentials?: Credential[];
  /** Settings shown on a sandbox's Config tab */
  config?: ConfigOption[];
  /** Script that removes the software again; without one, removing only forgets it */
  uninstall?: string | null;
  /** Recipes that are installed first */
  requires?: string[];
  /** For agents: the command that works through one task without a terminal */
  task?: string | null;
  /** For agents: the variable that names the model it starts with */
  model_env?: string | null;
  /** A single-colour SVG drawing, when the definition has one. */
  icon?: string | null;
  /** The brand's colour as `#rrggbb`. */
  icon_color?: string | null;
  id: string;
  /** The repository's name, or `local` */
  source: string;
  /** From one of the official repositories */
  official: boolean;
  install_allow: string[];
  /** System families the recipe supports; debian when empty */
  os?: string[];
  /** Versions on offer, the default first; empty when there is no choice. */
  versions: string[];
  name: string;
  category: "agent" | "language" | "tool" | "ide";
  description: string;
  allow: string[];
  secrets: string[];
  command: string | null;
}

export interface Repo {
  name: string;
  official: boolean;
  url: string;
  revision: string;
  recipes: number;
  /** Operating system definitions */
  systems?: number;
}

export interface Sample {
  ts_ms: number;
  /** Busy share of all CPUs, 0 to 100 */
  cpu: number;
  mem_used_mib: number;
  mem_total_mib: number;
  disk_used_gib: number;
  disk_total_gib: number;
  /** Bytes per second */
  rx_rate: number;
  tx_rate: number;
}

/** Per running sandbox: recent load samples and connection counts since its start. */
export type Stats = Record<string, { samples: Sample[]; allowed: number; blocked: number }>;

/** Instructions, MCP servers and skills for the agents: for all sandboxes, or for one. */
export interface AgentConfig {
  instructions: string;
  /** JSON text: name → server */
  mcp: string;
  skills: string[];
  path: string;
}

/** Something on this computer that could be handed to the sandboxes. */
export interface Found {
  name: string;
  origin: string;
  detail: string;
  has_secrets: boolean;
}

export interface Suggestions {
  instructions: Found[];
  skills: Found[];
  mcp: Found[];
}

export interface System {
  /** Versions of one system share a group: the id of its default version. */
  group?: string | null;
  version?: string | null;
  /** A single-colour SVG drawing, when the definition has one. */
  icon?: string | null;
  /** The brand's colour as `#rrggbb`. */
  icon_color?: string | null;
  id: string;
  name: string;
  description: string;
  family: string;
  built: boolean;
  /** The latest build of its image, if one ran since the service started */
  job?: Job | null;
}

export interface FolderListing {
  path: string;
  parent: string | null;
  folders: string[];
  home: string | null;
}

export interface HostInfo {
  cpus: number;
  /** Unknown on some systems */
  ram_mib: number | null;
  default_backend: "krun" | "qemu";
}

export interface AboutInfo {
  version: string;
  changelog: string;
  repository: string;
  license: string;
}

export interface UpdateInfo {
  current: string;
  latest: string | null;
  newer: boolean;
  url?: string;
  note?: string;
}

export interface Settings {
  notify_blocked: boolean;
  toast_blocked: boolean;
  /** Destinations that are blocked without telling anyone */
  ignore_blocked: string[];
  /** Web browsers may open the interface, not only the desktop app */
  browser_ui: boolean;
  /** Addresses besides 127.0.0.1 on which the UI is reachable */
  listen: string[];
  listen_port: number;
  /** Fetch the repositories once a day */
  update_repos: boolean;
  /** Look for a newer Boxquell once a day */
  check_updates: boolean;
  /** Stop a sandbox after this many minutes without work; 0 for never */
  idle_stop_minutes: number;
  /** What assistants connected through MCP may do */
  mcp_read_only: boolean;
  mcp_network_rules: boolean;
  mcp_delete: boolean;
  mcp_review_apply: boolean;
}

export interface Secret {
  name: string;
  hosts: string[];
  env: string[];
}

export interface NetEvent {
  seq: number;
  id: number;
  sandbox: string;
  ts_ms: number;
  kind: "dns" | "tcp" | "udp" | "http";
  phase: "once" | "open" | "close";
  host: string | null;
  ip: string | null;
  port: number | null;
  allowed: boolean;
  reason: string | null;
  bytes_up: number | null;
  bytes_down: number | null;
  duration_ms: number | null;
  method: string | null;
  path: string | null;
  status: number | null;
  secret: string | null;
}

async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method,
    headers: { Authorization: `Bearer ${token}`, ...(body === undefined ? {} : { "Content-Type": "application/json" }) },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error ?? `${response.status} ${response.statusText}`);
  return data as T;
}

/** A daemon older than this page leaves newer fields out; fill them so the page still works. */
function withDefaults(sandbox: Sandbox): Sandbox {
  return { ...sandbox, mode: sandbox.mode ?? "whitelist", recipes: sandbox.recipes ?? [], local_secrets: sandbox.local_secrets ?? [], ports: sandbox.ports ?? [], shares: sandbox.shares ?? [], workdir: sandbox.workdir ?? null, warnings: sandbox.warnings ?? [] };
}

const agentPath = (sandbox: string | null) => (sandbox ? `/api/sandboxes/${encodeURIComponent(sandbox)}/agent-config` : "/api/agent-config");
const box = (name: string) => `/api/sandboxes/${encodeURIComponent(name)}`;

/** SSH keys of this computer that sandboxes may sign with, and what there is to choose from. */
export type SshKeyChoice = {
  agent: boolean;
  files: string[];
  global: { agent: boolean; files: string[] };
  problems: string[];
  available: { agent: boolean; supported: boolean; keys: { path: string; kind: string; comment: string; passphrase: boolean }[] };
};

/** What this page needs from the daemon; see `API_LEVEL` in `crates/boxquell/src/api.rs`. */
export const API_LEVEL = 2;

export const api = {
  /** The API level of the running daemon; 0 for one that predates levels. */
  level: () => call<{ api?: number }>("GET", "/api/ping").then((ping) => ping.api ?? 0),
  sandboxes: () => call<Sandbox[]>("GET", "/api/sandboxes").then((list) => list.map(withDefaults)),
  recipes: () =>
    call<Recipe[]>("GET", "/api/recipes").then((list) => list.map((r) => ({ ...r, versions: r.versions ?? [], install_allow: r.install_allow ?? [] }))),
  repos: () => call<Repo[]>("GET", "/api/repos"),
  addRepo: (url: string, name?: string) => call<Repo>("POST", "/api/repos", { url, name }),
  updateRepo: (name: string) => call<Repo>("POST", `/api/repos/${encodeURIComponent(name)}`, {}),
  removeRepo: (name: string) => call<unknown>("DELETE", `/api/repos/${encodeURIComponent(name)}`),
  secrets: () => call<Secret[]>("GET", "/api/secrets"),
  create: (spec: Record<string, unknown>) => call<Sandbox>("POST", "/api/sandboxes", spec),
  start: (name: string) => call<Sandbox>("POST", `${box(name)}/start`, {}),
  pause: (name: string) => call<Sandbox>("POST", `${box(name)}/pause`, {}),
  resume: (name: string) => call<Sandbox>("POST", `${box(name)}/resume`, {}),
  stop: (name: string) => call<Sandbox>("POST", `${box(name)}/stop`, {}),
  remove: (name: string) => call<unknown>("DELETE", box(name)),
  resize: (name: string, change: { cpus?: number; ram_mib?: number; disk_gib?: number; workdir?: string; autostart?: string[]; env?: Record<string, string>; upload_limit_mib?: number; download_limit_mib?: number; idle_stop_minutes?: number; upload_kbit?: number; download_kbit?: number }) => call<Sandbox>("PATCH", box(name), change),
  addShare: (name: string, host: string, guest: string, read_only: boolean) => call<Sandbox>("POST", `${box(name)}/shares`, { host, guest, read_only }),
  removeShare: (name: string, guest: string) => call<Sandbox>("DELETE", `${box(name)}/shares`, { guest }),
  addPort: (name: string, host: number, guest: number) => call<Sandbox>("POST", `${box(name)}/ports`, { host, guest }),
  removePort: (name: string, host: number) => call<Sandbox>("DELETE", `${box(name)}/ports/${host}`),
  setLocalSecret: (name: string, secret: string, value: string, hosts?: string[], env?: string[]) =>
    call<Sandbox>("PUT", `${box(name)}/secrets/${encodeURIComponent(secret)}`, { value, hosts, env }),
  removeLocalSecret: (name: string, secret: string) => call<Sandbox>("DELETE", `${box(name)}/secrets/${encodeURIComponent(secret)}`),
  setClaudeSettings: (name: string, settings: string) => call<Sandbox>("PUT", `${box(name)}/claude-settings`, { settings }),
  /** Take over this computer's managed Claude Code settings; they are read by the daemon, not sent from here. */
  useHostClaudeSettings: (name: string) => call<Sandbox>("PUT", `${box(name)}/claude-settings`, { host: true }),
  hostClaudeSettings: () => call<{ path: string; present: boolean }>("GET", "/api/claude-settings/host"),
  clearClaudeSettings: (name: string) => call<Sandbox>("DELETE", `${box(name)}/claude-settings`),
  agentConfig: (sandbox: string | null) => call<AgentConfig>("GET", agentPath(sandbox)),
  setAgentConfig: (sandbox: string | null, instructions: string, mcp: string) => call<AgentConfig>("PUT", agentPath(sandbox), { instructions, mcp }),
  addSkill: (sandbox: string | null, path: string) => call<AgentConfig>("POST", `${agentPath(sandbox)}/skills`, { path }),
  removeSkill: (sandbox: string | null, skill: string) => call<AgentConfig>("DELETE", `${agentPath(sandbox)}/skills/${encodeURIComponent(skill)}`),
  agentSuggestions: (sandbox: string | null) => call<Suggestions>("GET", `/api/agent-suggestions${sandbox ? `?sandbox=${encodeURIComponent(sandbox)}` : ""}`),
  agentImport: (sandbox: string | null, items: { instructions?: string[]; skills?: string[]; mcp?: string[] }) =>
    call<AgentConfig>("POST", `/api/agent-suggestions${sandbox ? `?sandbox=${encodeURIComponent(sandbox)}` : ""}`, items),
  folders: (path?: string, hidden = false) => call<FolderListing>("GET", `/api/folders?hidden=${hidden}${path ? `&path=${encodeURIComponent(path)}` : ""}`),
  /** Open a web address in the user's browser (needed inside the desktop app). */
  openLink: (url: string) => call<unknown>("POST", "/api/open", { url }),
  host: () => call<HostInfo>("GET", "/api/host"),
  about: () => call<AboutInfo>("GET", "/api/about"),
  notices: () => call<{ text: string }>("GET", "/api/notices"),
  updateCheck: () => call<UpdateInfo>("GET", "/api/update-check"),
  /** What the daemon's daily look for a newer release found; `null` before the first one. */
  updateStatus: () => call<UpdateInfo | null>("GET", "/api/update-status"),
  daemonLog: () => call<{ text: string }>("GET", "/api/logs/daemon"),
  consoleLog: (name: string) => call<{ text: string }>("GET", `${box(name)}/console`),
  buildSystem: (id: string) => call<unknown>("POST", `/api/os/${encodeURIComponent(id)}/build`, {}),
  systems: () => call<System[]>("GET", "/api/os"),
  stats: () => call<Stats>("GET", "/api/stats"),
  /** The load of this computer itself, shaped like a sandbox's measurements. */
  hostStats: () => call<{ cpus: number; samples: Sample[] }>("GET", "/api/host-load"),
  lockdown: () => call<{ on: boolean }>("GET", "/api/lockdown"),
  setLockdown: (on: boolean) => call<{ on: boolean }>("POST", "/api/lockdown", { on }),
  sshKeys: (sandbox: string | null) => call<SshKeyChoice>("GET", sandbox ? `${box(sandbox)}/ssh-keys` : "/api/ssh-keys"),
  setSshKeys: (sandbox: string | null, keys: { agent: boolean; files: string[] }) =>
    call<SshKeyChoice>("PUT", sandbox ? `${box(sandbox)}/ssh-keys` : "/api/ssh-keys", keys),
  settings: () => call<Settings>("GET", "/api/settings").then((s) => ({ ...s, ignore_blocked: s.ignore_blocked ?? [], browser_ui: s.browser_ui ?? true, listen: s.listen ?? [], listen_port: s.listen_port ?? 7878, update_repos: s.update_repos ?? true, check_updates: s.check_updates ?? true, idle_stop_minutes: s.idle_stop_minutes ?? 0, mcp_read_only: !!s.mcp_read_only, mcp_network_rules: !!s.mcp_network_rules, mcp_delete: !!s.mcp_delete, mcp_review_apply: !!s.mcp_review_apply })),
  addresses: () => call<{ address: string; interface: string }[]>("GET", "/api/addresses"),
  setSettings: (settings: Settings) => call<Settings>("PUT", "/api/settings", settings),
  /** The most recent blocked connections of every sandbox. */
  blockedEvents: () => call<NetEvent[]>("GET", "/api/events?blocked_only=true&limit=500"),
  /** Blocked connections of every sandbox, live; returns a function that ends the stream. */
  followBlocked: (onEvent: (event: NetEvent) => void) => {
    const source = new EventSource(`/api/events/stream?blocked_only=true&token=${encodeURIComponent(token)}`);
    source.onmessage = (message) => onEvent(JSON.parse(message.data));
    return () => source.close();
  },
  /** Blocked destinations that were dismissed or allowed: `<sandbox> <destination>` with the time up to which they count as seen. */
  handledNotifications: () => call<Record<string, number>>("GET", "/api/notifications/handled"),
  markHandled: (items: { key: string; until: number }[]) => call<Record<string, number>>("POST", "/api/notifications/handled", { items }),
  globalRules: () => call<string[]>("GET", "/api/rules"),
  addGlobalRule: (rule: string) => call<string[]>("POST", "/api/rules", { rule }),
  removeGlobalRule: (rule: string) => call<string[]>("DELETE", "/api/rules", { rule }),
  setMode: (name: string, mode: string) => call<Sandbox>("POST", `${box(name)}/mode`, { mode }),
  /** Add a rule; with `minutes` it ends by itself. */
  addRule: (name: string, rule: string, minutes?: number) => call<Sandbox>("POST", `${box(name)}/rules`, { rule, minutes }),
  /** Let a sandbox that reached a data limit go on; counting starts anew. */
  continueAfterLimit: (name: string) => call<Sandbox>("POST", `${box(name)}/traffic/continue`, {}),
  /** Switch a destination that software or the system opens, or all of a source's, on or off. */
  setAutoRule: (name: string, source: string, host: string | null, enabled: boolean) => call<Sandbox>("POST", `${box(name)}/auto-rules`, { source, host, enabled }),
  removeRule: (name: string, rule: string) => call<Sandbox>("DELETE", `${box(name)}/rules`, { rule }),
  install: (name: string, recipes: string[], again = false) => call<Sandbox>("POST", `${box(name)}/recipes`, { recipes, again }),
  takeSnapshot: (name: string, label: string) => call<Sandbox>("POST", `${box(name)}/snapshots`, { label }),
  restoreSnapshot: (name: string, id: string) => call<Sandbox>("POST", `${box(name)}/snapshots/${encodeURIComponent(id)}`, {}),
  deleteSnapshot: (name: string, id: string) => call<Sandbox>("DELETE", `${box(name)}/snapshots/${encodeURIComponent(id)}`),
  templates: () => call<Template[]>("GET", "/api/templates"),
  /** Where the Boxquell program is, and whether Claude Code is here and connected. */
  mcp: () => call<{ exe: string | null; claude: boolean; claude_connected: boolean }>("GET", "/api/mcp"),
  connectMcp: (connect: boolean) => call<{ exe: string | null; claude: boolean; claude_connected: boolean }>("POST", "/api/mcp", { connect }),
  /** A sandbox's traffic to and from destinations outside: per hour or day, and the destinations that carried most of it. */
  traffic: (name: string, days: number) =>
    call<{ bucket_ms: number; buckets: { t: number; up: number; down: number }[]; top: { host: string | null; up: number; down: number; connections: number }[] }>("GET", `${box(name)}/traffic?days=${days}`),
  setReview: (name: string, on: boolean) => call<Sandbox>("PUT", `${box(name)}/review`, { on }),
  review: (name: string) => call<{ project: string; changes: Change[] }>("GET", `${box(name)}/review`),
  reviewDiff: (name: string, path: string, towardsProject: boolean) =>
    call<{ rows: DiffRow[]; binary?: boolean; large?: boolean; cut?: boolean }>("GET", `${box(name)}/review/diff?towards_project=${towardsProject}&path=${encodeURIComponent(path)}`),
  /** `apply` takes the sandbox's files into the project, `discard` undoes them in its copy, `bring` takes the project's newer files into the copy. */
  settleReview: (name: string, action: "apply" | "discard" | "bring", paths?: string[]) =>
    call<{ done: number; project: string; changes: Change[] }>("POST", `${box(name)}/review`, { action, paths }),
  startTask: (name: string, agent: string, prompt: string, snapshot: boolean) => call<Task>("POST", `${box(name)}/tasks`, { agent, prompt, snapshot }),
  taskLog: (name: string, id: string) => call<{ text: string }>("GET", `${box(name)}/tasks/${encodeURIComponent(id)}`),
  stopTask: (name: string, id: string) => call<unknown>("POST", `${box(name)}/tasks/${encodeURIComponent(id)}/stop`, {}),
  deleteTask: (name: string, id: string) => call<Sandbox>("DELETE", `${box(name)}/tasks/${encodeURIComponent(id)}`),
  usage: (days: number) => call<UsageReport>("GET", `/api/usage?days=${days}`),
  prices: () => call<Price[]>("GET", "/api/prices"),
  setPrices: (prices: Price[]) => call<Price[]>("PUT", "/api/prices", prices),
  keyLimits: () => call<Record<string, { daily_usd?: number; daily_tokens?: number }>>("GET", "/api/key-limits"),
  setKeyLimits: (limits: Record<string, { daily_usd?: number; daily_tokens?: number }>) => call<unknown>("PUT", "/api/key-limits", limits),
  /** The `boxquell.toml` of a folder, or `null` when it has none. */
  projectFile: (dir: string) => call<{ file: string; name: string; project: ProjectFile } | null>("GET", `/api/project?dir=${encodeURIComponent(dir)}`),
  /** Give a sandbox what the project file of `dir` names beyond what creating it covers. */
  applyProject: (name: string, dir: string) => call<Sandbox>("POST", `${box(name)}/project`, { dir }),
  /** Write the sandbox's description as `boxquell.toml` into its project folder. */
  writeProjectFile: (name: string, force: boolean) => call<{ file: string }>("POST", `${box(name)}/project`, { force }),
  saveTemplate: (name: string, template: string, description: string) => call<Template>("POST", `${box(name)}/template`, { name: template, description }),
  deleteTemplate: (template: string) => call<Template[]>("DELETE", `/api/templates/${encodeURIComponent(template)}`),
  /** Take software out of a sandbox again; like an install, it runs in the background. */
  uninstall: (name: string, recipes: string[]) => call<Sandbox>("DELETE", `${box(name)}/recipes`, { recipes }),
  attachSecret: (name: string, secret: string) => call<Sandbox>("POST", `${box(name)}/secrets`, { secret }),
  setSecret: (name: string, value: string, hosts?: string[], env?: string[]) =>
    call<Secret>("PUT", `/api/secrets/${encodeURIComponent(name)}`, { value, hosts, env }),
  removeSecret: (name: string) => call<unknown>("DELETE", `/api/secrets/${encodeURIComponent(name)}`),
  events: (sandbox: string, limit = 300) => call<NetEvent[]>("GET", `/api/events?sandbox=${encodeURIComponent(sandbox)}&limit=${limit}`),
  /** Live events for one sandbox; returns a function that ends the stream. */
  follow: (sandbox: string, onEvent: (event: NetEvent) => void) => {
    const source = new EventSource(`/api/events/stream?sandbox=${encodeURIComponent(sandbox)}&token=${encodeURIComponent(token)}`);
    source.onmessage = (message) => onEvent(JSON.parse(message.data));
    return () => source.close();
  },
};
