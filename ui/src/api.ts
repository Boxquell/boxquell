// Client for the daemon API. The token arrives in the URL fragment (`boxquell ui`)
// and is kept for this browser tab only.

const fromHash = new URLSearchParams(location.hash.slice(1)).get("token");
if (fromHash) {
  sessionStorage.setItem("boxquell-token", fromHash);
  history.replaceState(null, "", location.pathname);
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
  mode: "whitelist" | "blacklist";
  rules: string[];
  secrets: string[];
  recipes: string[];
  local_secrets: Secret[];
  claude_settings: boolean;
  ports: { host: number; guest: number }[];
  /** What keeps installed agents from working, e.g. a missing credential. */
  warnings: string[];
  /** Present in the answer to changing the Claude settings. */
  notes?: string[];
  job: Job | null;
}

export interface Recipe {
  id: string;
  /** The repository's name, or `local` */
  source: string;
  /** From one of the official repositories */
  official: boolean;
  install_allow: string[];
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
}

export interface Settings {
  notify_blocked: boolean;
  toast_blocked: boolean;
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
  return { ...sandbox, mode: sandbox.mode ?? "whitelist", recipes: sandbox.recipes ?? [], local_secrets: sandbox.local_secrets ?? [], ports: sandbox.ports ?? [], warnings: sandbox.warnings ?? [] };
}

const box = (name: string) => `/api/sandboxes/${encodeURIComponent(name)}`;

export const api = {
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
  resize: (name: string, change: { cpus?: number; ram_mib?: number; disk_gib?: number }) => call<Sandbox>("PATCH", box(name), change),
  addPort: (name: string, host: number, guest: number) => call<Sandbox>("POST", `${box(name)}/ports`, { host, guest }),
  removePort: (name: string, host: number) => call<Sandbox>("DELETE", `${box(name)}/ports/${host}`),
  setLocalSecret: (name: string, secret: string, value: string, hosts: string[], env: string[]) =>
    call<Sandbox>("PUT", `${box(name)}/secrets/${encodeURIComponent(secret)}`, { value, hosts, env }),
  removeLocalSecret: (name: string, secret: string) => call<Sandbox>("DELETE", `${box(name)}/secrets/${encodeURIComponent(secret)}`),
  setClaudeSettings: (name: string, settings: string) => call<Sandbox>("PUT", `${box(name)}/claude-settings`, { settings }),
  clearClaudeSettings: (name: string) => call<Sandbox>("DELETE", `${box(name)}/claude-settings`),
  lockdown: () => call<{ on: boolean }>("GET", "/api/lockdown"),
  setLockdown: (on: boolean) => call<{ on: boolean }>("POST", "/api/lockdown", { on }),
  settings: () => call<Settings>("GET", "/api/settings"),
  setSettings: (settings: Settings) => call<Settings>("PUT", "/api/settings", settings),
  /** Blocked connections of every sandbox, live; returns a function that ends the stream. */
  followBlocked: (onEvent: (event: NetEvent) => void) => {
    const source = new EventSource(`/api/events/stream?blocked_only=true&token=${encodeURIComponent(token)}`);
    source.onmessage = (message) => onEvent(JSON.parse(message.data));
    return () => source.close();
  },
  globalRules: () => call<string[]>("GET", "/api/rules"),
  addGlobalRule: (rule: string) => call<string[]>("POST", "/api/rules", { rule }),
  removeGlobalRule: (rule: string) => call<string[]>("DELETE", "/api/rules", { rule }),
  setMode: (name: string, mode: string) => call<Sandbox>("POST", `${box(name)}/mode`, { mode }),
  addRule: (name: string, rule: string) => call<Sandbox>("POST", `${box(name)}/rules`, { rule }),
  removeRule: (name: string, rule: string) => call<Sandbox>("DELETE", `${box(name)}/rules`, { rule }),
  install: (name: string, recipes: string[]) => call<Sandbox>("POST", `${box(name)}/recipes`, { recipes }),
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
