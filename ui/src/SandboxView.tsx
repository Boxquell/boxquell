import { useEffect, useRef, useState, type FormEvent } from "react";
import { Activity, Bot, Network, Package, Pause, Play, ScrollText, Settings2, Square, SquareTerminal, KeyRound, type LucideIcon } from "lucide-react";
import { api, type Recipe, type Sandbox, type Secret } from "./api";
import { FolderField } from "./FolderPicker";
import { LogViewer } from "./LogViewer";
import { MachineControls } from "./Machine";
import { NetworkLog } from "./NetworkLog";
import { Tile, VmLoad, useStats } from "./Overview";
import { AgentSettings } from "./AgentSettings";
import { RecipeCards, recipeName } from "./RecipeCards";

type Act = (action: () => Promise<unknown>) => Promise<void>;

type OpenTerminal = (sandbox: string, title: string, cmd?: string) => void;

export type Tab = "overview" | "terminal" | "software" | "agent" | "network" | "log" | "secrets" | "settings";
const TABS: [Tab, string][] = [
  ["overview", "Overview"],
  ["terminal", "Terminal"],
  ["software", "Software"],
  ["agent", "Agent"],
  ["network", "Network"],
  ["log", "Log"],
  ["secrets", "Secrets"],
  ["settings", "Settings"],
];

const TAB_ICONS: Record<Tab, LucideIcon> = {
  overview: Activity,
  terminal: SquareTerminal,
  software: Package,
  agent: Bot,
  network: Network,
  log: ScrollText,
  secrets: KeyRound,
  settings: Settings2,
};

interface ViewProps {
  sandbox: Sandbox;
  recipes: Recipe[];
  secrets: Secret[];
  act: Act;
  openTerminal: OpenTerminal;
  tab: Tab;
  setTab: (tab: Tab) => void;
}

export function SandboxView(props: ViewProps) {
  const { sandbox, recipes, act, openTerminal, tab, setTab } = props;
  const [busy, setBusy] = useState<string | null>(null);
  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(label);
    await act(action);
    setBusy(null);
  };
  const agents = sandbox.recipes.map((id) => recipes.find((r) => r.id === recipeName(id))).filter((r) => r?.command);

  return (
    <>
      <header className="hero">
        <div className="grow">
          <h2>
            <span className={sandbox.paused ? "dot paused" : sandbox.running ? "dot on" : "dot"} /> {sandbox.name}
          </h2>
          <p className="muted">
            <span className={`chip ${sandbox.paused ? "paused" : sandbox.running ? "on" : ""}`}>{busy ?? (sandbox.paused ? "paused" : sandbox.running ? "running" : "stopped")}</span>{" "}
            <span className="chip">{sandbox.os ?? "debian"}</span> {sandbox.backend} · {sandbox.cpus} CPUs · {sandbox.ram_mib} MiB · {sandbox.disk_gib} GiB disk
            {sandbox.workspace && <> · {sandbox.workspace}</>}
          </p>
        </div>
        {sandbox.running &&
          (sandbox.paused ? (
            <button className="primary" disabled={!!busy} onClick={() => run("resuming…", () => api.resume(sandbox.name))}>
              Resume
            </button>
          ) : (
            <button disabled={!!busy} title="Freeze the sandbox: it keeps its memory and programs but uses no CPU" onClick={() => run("pausing…", () => api.pause(sandbox.name))}>
              <Pause size={15} aria-hidden="true" /> Pause
            </button>
          ))}
        {sandbox.running ? (
          <button disabled={!!busy} onClick={() => run("stopping…", () => api.stop(sandbox.name))}>
            <Square size={14} aria-hidden="true" /> Stop
          </button>
        ) : (
          <button className="primary" disabled={!!busy} onClick={() => run("starting…", () => api.start(sandbox.name))}>
            <Play size={15} aria-hidden="true" /> Start
          </button>
        )}
      </header>

      {sandbox.warnings.length > 0 && (
        <div className="warning" role="status">
          {sandbox.warnings.map((warning) => (
            <div key={warning}>{warning}</div>
          ))}
          <button className="link" onClick={() => setTab("secrets")}>
            Open secrets
          </button>
        </div>
      )}

      <div className="tabbar" role="tablist">
        {TABS.map(([id, label]) => (
          <button key={id} role="tab" aria-selected={tab === id} className={tab === id ? "active" : ""} onClick={() => setTab(id)}>
            <TabIcon tab={id} />
            {label}
            {id === "network" && sandbox.mode === "blacklist" && <small>blacklist</small>}
          </button>
        ))}
      </div>

      {tab === "overview" && <Status sandbox={sandbox} recipes={recipes} start={() => run("starting…", () => api.start(sandbox.name))} setTab={setTab} />}
      {tab === "terminal" && (
      <section>
          <h3>Terminal</h3>
          <div className="actions left">
            <button className="primary" onClick={() => openTerminal(sandbox.name, "Shell")}>
              Shell
            </button>
            {agents.map((r) => (
              <button key={r!.id} onClick={() => openTerminal(sandbox.name, r!.name, r!.command!)}>
                {r!.name}
              </button>
            ))}
            {!sandbox.running && <span className="muted">Opening a terminal starts the sandbox.</span>}
          </div>
          <p className="muted">Or from your own terminal:</p>
          <div className="commands">
            <Command text={`boxquell shell ${sandbox.name}`} />
            {agents.map((r) => (
              <Command key={r!.id} text={`boxquell run ${sandbox.name} ${r!.id.split(":").pop()}`} />
            ))}
          </div>
        </section>
      )}
      {tab === "software" && <Software {...props} />}
      {tab === "agent" && <AgentSettings sandbox={sandbox.name} act={act} />}
      {tab === "network" && (
        <>
          <Rules sandbox={sandbox} act={act} />
          <Ports sandbox={sandbox} act={act} />
          <GlobalRules act={act} />
        </>
      )}
      {tab === "log" && <NetworkLog sandbox={sandbox} act={act} />}
      {tab === "secrets" && (
        <>
          <SandboxSecrets {...props} />
          <ClaudeSettings sandbox={sandbox} act={act} />
        </>
      )}
      {tab === "settings" && <Resources sandbox={sandbox} recipes={recipes} act={act} remove={() => run("deleting…", () => api.remove(sandbox.name))} />}
    </>
  );
}

function TabIcon(props: { tab: Tab }) {
  const Icon = TAB_ICONS[props.tab];
  return <Icon size={15} aria-hidden="true" />;
}

function Command(props: { text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <button
      className="command"
      title="Copy"
      onClick={() => navigator.clipboard.writeText(props.text).then(() => (setCopied(true), setTimeout(() => setCopied(false), 1200)))}
    >
      <code>{props.text}</code>
      <small>{copied ? "copied" : "copy"}</small>
    </button>
  );
}

function Software(props: ViewProps) {
  const { sandbox, recipes, secrets, act } = props;
  const [picked, setPicked] = useState<string[]>([]);
  const job = sandbox.job;
  const log = useRef<HTMLPreElement>(null);
  useEffect(() => {
    if (log.current) log.current.scrollTop = log.current.scrollHeight;
  }, [job?.log]);

  // Secrets the installed software can use that are stored but not attached yet, or not stored at all.
  const wanted = [...new Set(sandbox.recipes.flatMap((id) => recipes.find((r) => r.id === recipeName(id))?.secrets ?? []))];
  const attachable = wanted.filter((s) => secrets.some((x) => x.name === s) && !sandbox.secrets.includes(s));
  const needsSecret = sandbox.recipes
    .map((id) => recipes.find((r) => r.id === recipeName(id)))
    .filter((r) => r && r.secrets.length > 0 && !r.secrets.some((s) => sandbox.secrets.includes(s)));

  return (
    <section className="software">
      <RecipeCards recipes={recipes} installed={sandbox.recipes} picked={picked} busy={job?.state === "running"} onChange={setPicked} />
      <div className="install-bar">
        <span className="grow muted">
          {job?.state === "running"
            ? `Installing ${job.recipes.join(", ")}…`
            : picked.length > 0
              ? `Selected: ${picked.map((p) => p.split(":").pop()).join(", ")}`
              : "Select what this sandbox should have. What a recipe needs is installed with it."}
        </span>
        {attachable.map((s) => (
          <button key={s} onClick={() => act(() => api.attachSecret(sandbox.name, s))}>
            Use secret {s}
          </button>
        ))}
        <button
          className="primary"
          disabled={picked.length === 0 || job?.state === "running"}
          onClick={() => act(() => api.install(sandbox.name, picked)).then(() => setPicked([]))}
        >
          {job?.state === "running" ? "Installing…" : picked.length > 0 ? `Install ${picked.length}` : "Install"}
        </button>
      </div>
      {needsSecret.map((r) => (
        <p key={r!.id} className="muted">
          {r!.name} has no credential here. Store one of [{r!.secrets.join(", ")}] under Secrets, then attach it.
        </p>
      ))}
      {sandbox.secrets.length > 0 && (
        <p className="muted">
          Secrets in use: {sandbox.secrets.join(", ")}
          {sandbox.running && " (a newly attached secret needs a restart)"}
        </p>
      )}
      {job && (
        <details open={job.state !== "done"}>
          <summary className={job.state === "failed" ? "bad" : ""}>
            Install of {job.recipes.join(", ")}: {job.state}
          </summary>
          <pre ref={log}>{job.log}</pre>
        </details>
      )}
    </section>
  );
}

function Rules(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [host, setHost] = useState("");
  const [action, setAction] = useState("allow");
  const submit = (event: FormEvent) => {
    event.preventDefault();
    act(() => api.addRule(sandbox.name, `${action} ${host.trim()}`)).then(() => setHost(""));
  };
  return (
    <section>
      <h3>Network rules</h3>
      <div className="row">
        <select value={sandbox.mode} onChange={(e) => act(() => api.setMode(sandbox.name, e.target.value))} aria-label="Network mode">
          <option value="whitelist">Whitelist: block everything that is not allowed</option>
          <option value="blacklist">Blacklist: allow everything that is not denied</option>
        </select>
      </div>
      <p className="muted">
        First match wins; address rules are checked before name rules.
        {sandbox.mode === "blacklist" && " Private and local addresses stay blocked unless a rule names them."}
      </p>
      <ul className="rules">
        {sandbox.rules.map((rule) => (
          <li key={rule} className={rule.startsWith("deny") ? "bad" : ""}>
            <code>{rule}</code>
            <button className="link" onClick={() => act(() => api.removeRule(sandbox.name, rule))}>
              remove
            </button>
          </li>
        ))}
      </ul>
      <form className="row" onSubmit={submit}>
        <select value={action} onChange={(e) => setAction(e.target.value)} aria-label="Action">
          <option>allow</option>
          <option>deny</option>
        </select>
        <input className="grow" required placeholder="example.com, *.example.com, 192.0.2.7, 10.0.0.0/8, optionally :port" value={host} onChange={(e) => setHost(e.target.value)} aria-label="Destination" />
        <button>Add rule</button>
      </form>
    </section>
  );
}

function Ports(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [guest, setGuest] = useState("");
  const [host, setHost] = useState("");
  const submit = (event: FormEvent) => {
    event.preventDefault();
    act(() => api.addPort(sandbox.name, +(host || guest), +guest)).then(() => (setGuest(""), setHost("")));
  };
  return (
    <section>
      <h3>Port forwards</h3>
      <p className="muted">Open something that listens inside the sandbox, such as a dev server or an agent's web page, from this computer.</p>
      <ul className="rules">
        {sandbox.ports.map((p) => (
          <li key={p.host}>
            <a href={`http://${p.bind ?? "127.0.0.1"}:${p.host}`} target="_blank" rel="noreferrer">
              {p.bind ?? "127.0.0.1"}:{p.host}
            </a>{" "}
            <span className="muted">→ sandbox port {p.guest}</span>
            <button className="link" onClick={() => act(() => api.removePort(sandbox.name, p.host))}>
              remove
            </button>
          </li>
        ))}
      </ul>
      <form className="row" onSubmit={submit}>
        <label>
          Port in the sandbox
          <input type="number" required min={1} max={65535} placeholder="3000" value={guest} onChange={(e) => setGuest(e.target.value)} />
        </label>
        <label>
          Port on this computer
          <input type="number" min={1024} max={65535} placeholder={guest || "same"} value={host} onChange={(e) => setHost(e.target.value)} />
        </label>
        <button>Forward</button>
      </form>
    </section>
  );
}

export function GlobalRules(props: { act: Act }) {
  const [rules, setRules] = useState<string[]>([]);
  const [host, setHost] = useState("");
  const [action, setAction] = useState("deny");
  useEffect(() => {
    api.globalRules().then(setRules, () => {});
  }, []);
  const change = (request: () => Promise<string[]>) => props.act(() => request().then(setRules));
  return (
    <section>
      <h3>Rules for all sandboxes</h3>
      <p className="muted">A global deny cannot be undone by a sandbox. A global allow applies everywhere unless a sandbox denies it.</p>
      <ul className="rules">
        {rules.map((rule) => (
          <li key={rule} className={rule.startsWith("deny") ? "bad" : ""}>
            <code>{rule}</code>
            <button className="link" onClick={() => change(() => api.removeGlobalRule(rule))}>
              remove
            </button>
          </li>
        ))}
        {rules.length === 0 && <li className="muted">none</li>}
      </ul>
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          change(() => api.addGlobalRule(`${action} ${host.trim()}`)).then(() => setHost(""));
        }}
      >
        <select value={action} onChange={(e) => setAction(e.target.value)} aria-label="Action">
          <option>deny</option>
          <option>allow</option>
        </select>
        <input className="grow" required placeholder="example.com, *.example.com, 192.0.2.7, 10.0.0.0/8" value={host} onChange={(e) => setHost(e.target.value)} aria-label="Destination" />
        <button>Add for all</button>
      </form>
    </section>
  );
}

function SandboxSecrets(props: ViewProps) {
  const { sandbox, secrets, act } = props;
  const [name, setName] = useState("");
  const [hosts, setHosts] = useState("");
  const [env, setEnv] = useState("");
  const [value, setValue] = useState("");
  const list = (text: string) => text.split(/[\s,]+/).filter(Boolean);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    act(() => api.setLocalSecret(sandbox.name, name, value, list(hosts), list(env))).then(() => (setName(""), setHosts(""), setEnv(""), setValue("")));
  };
  const attachable = secrets.filter((s) => !sandbox.secrets.includes(s.name));
  return (
    <section>
      <h3>Secrets of this sandbox</h3>
      <p className="muted">
        Values stay on this computer. The sandbox sees a placeholder; the real value is put into requests to the listed hosts on their way out.
        {sandbox.running && " A new secret's variable appears after a restart."}
      </p>
      <table>
        <tbody>
          {sandbox.local_secrets.map((s) => (
            <tr key={s.name}>
              <td>
                <strong>{s.name}</strong> <span className="chip">only here</span>
              </td>
              <td className="muted">{s.hosts.join(", ")}</td>
              <td className="muted">{s.env.join(", ")}</td>
              <td className="right">
                <button className="link danger" onClick={() => confirm(`Delete the secret ${s.name} of ${sandbox.name}?`) && act(() => api.removeLocalSecret(sandbox.name, s.name))}>
                  delete
                </button>
              </td>
            </tr>
          ))}
          {sandbox.secrets.map((s) => (
            <tr key={s}>
              <td>
                <strong>{s}</strong> <span className="chip">shared</span>
              </td>
              <td className="muted" colSpan={3}>
                {secrets.find((x) => x.name === s)?.hosts.join(", ")}
              </td>
            </tr>
          ))}
          {sandbox.local_secrets.length + sandbox.secrets.length === 0 && (
            <tr>
              <td className="muted">This sandbox has no secrets.</td>
            </tr>
          )}
        </tbody>
      </table>
      {attachable.length > 0 && (
        <div className="actions left">
          {attachable.map((s) => (
            <button key={s.name} onClick={() => act(() => api.attachSecret(sandbox.name, s.name))}>
              Use shared secret {s.name}
            </button>
          ))}
        </div>
      )}
      <form className="row" onSubmit={submit}>
        <label>
          Name
          <input required pattern="[a-z0-9][a-z0-9-]*" placeholder="my-api" value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="grow">
          Hosts that may receive it
          <input required placeholder="api.example.com" value={hosts} onChange={(e) => setHosts(e.target.value)} />
        </label>
        <label>
          Variable in the sandbox
          <input placeholder="EXAMPLE_API_KEY" value={env} onChange={(e) => setEnv(e.target.value)} />
        </label>
        <label>
          Value
          <input type="password" required autoComplete="off" value={value} onChange={(e) => setValue(e.target.value)} />
        </label>
        <button className="primary">Save</button>
      </form>
    </section>
  );
}

function ClaudeSettings(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [text, setText] = useState("");
  const [notes, setNotes] = useState<string[]>([]);
  return (
    <section>
      <h3>
        Claude Code settings <span className={sandbox.claude_settings ? "chip on" : "chip"}>{sandbox.claude_settings ? "in use" : "none"}</span>
      </h3>
      <p className="muted">
        Paste a Claude Code settings file, for example one that points at a LiteLLM gateway (ANTHROPIC_BASE_URL, ANTHROPIC_AUTH_TOKEN, model names). The
        sandbox gets it as its managed settings. Keys in it stay on this computer as secrets of this sandbox, and the gateway is allowed in the network
        rules. Applies from the next start.
      </p>
      <textarea
        rows={6}
        spellCheck={false}
        placeholder={'{\n  "env": {\n    "ANTHROPIC_BASE_URL": "https://llm.example.com",\n    "ANTHROPIC_AUTH_TOKEN": "sk-…"\n  }\n}'}
        value={text}
        onChange={(e) => setText(e.target.value)}
      />
      {notes.map((note) => (
        <p key={note} className="muted">
          {note}
        </p>
      ))}
      <div className="actions left">
        <button
          className="primary"
          disabled={!text.trim()}
          onClick={() =>
            act(() =>
              api.setClaudeSettings(sandbox.name, text).then((info) => {
                setNotes(info.notes ?? []);
                setText("");
              }),
            )
          }
        >
          Use these settings
        </button>
        {sandbox.claude_settings && <button onClick={() => act(() => api.clearClaudeSettings(sandbox.name)).then(() => setNotes([]))}>Remove settings</button>}
      </div>
    </section>
  );
}

function Resources(props: { sandbox: Sandbox; recipes: Recipe[]; act: Act; remove: () => void }) {
  const { sandbox, recipes, act } = props;
  const [cpus, setCpus] = useState(sandbox.cpus);
  const [ram, setRam] = useState(sandbox.ram_mib);
  const [disk, setDisk] = useState(sandbox.disk_gib);
  const changed = cpus !== sandbox.cpus || ram !== sandbox.ram_mib || disk !== sandbox.disk_gib;
  return (
    <>
      <section>
        <h3>Machine</h3>
        <MachineControls
          value={{ cpus, ram, disk }}
          minDisk={sandbox.disk_gib}
          onChange={(next) => {
            setCpus(next.cpus);
            setRam(next.ram);
            setDisk(next.disk);
          }}
        />
        <div className="actions left">
          <button className="primary" disabled={!changed} onClick={() => act(() => api.resize(sandbox.name, { cpus, ram_mib: ram, disk_gib: disk }))}>
            Save
          </button>
          {changed && (
            <button
              onClick={() => {
                setCpus(sandbox.cpus);
                setRam(sandbox.ram_mib);
                setDisk(sandbox.disk_gib);
              }}
            >
              Discard changes
            </button>
          )}
        </div>
        {sandbox.running && <p className="muted">Changes apply the next time the sandbox starts.</p>}
        <p className="muted">
          Backend {sandbox.backend}
          {sandbox.workspace ? ` · project folder ${sandbox.workspace}` : " · no project folder"}
        </p>
      </section>
      <Autostart sandbox={sandbox} recipes={recipes} act={act} />
      <LogViewer
        key={sandbox.name}
        title="Console log"
        about="What the sandbox printed while starting and running: messages of its kernel and start script. Useful when it does not come up."
        load={() => api.consoleLog(sandbox.name)}
      />
      <Folders sandbox={sandbox} act={act} />
      <section>
        <h3>Delete</h3>
        <p className="muted">Removes the sandbox, its disk and its secrets. The project folder is kept.</p>
        <div className="actions left">
          <button className="danger" onClick={() => confirm(`Delete ${sandbox.name} and its disk? The project folder is kept.`) && props.remove()}>
            Delete {sandbox.name}
          </button>
        </div>
      </section>
    </>
  );
}

function Folders(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [workdir, setWorkdir] = useState(sandbox.workdir ?? "");
  const [host, setHost] = useState("");
  const [guest, setGuest] = useState("");
  const [readOnly, setReadOnly] = useState(false);
  return (
    <section>
      <h3>Folders</h3>
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          act(() => api.resize(sandbox.name, { workdir: workdir.trim() || "default" }));
        }}
      >
        <label className="grow">
          Project root: where shells and agents start in the sandbox
          <input placeholder="/workspace" value={workdir} onChange={(e) => setWorkdir(e.target.value)} />
        </label>
        <button disabled={(sandbox.workdir ?? "") === workdir.trim()}>Save</button>
      </form>
      <table>
        <tbody>
          <tr>
            <td>
              <code>/workspace</code> <span className="chip">project folder</span>
            </td>
            <td className="muted" colSpan={2}>
              {sandbox.workspace ?? "not shared"}
            </td>
          </tr>
          {sandbox.shares.map((share) => (
            <tr key={share.guest}>
              <td>
                <code>{share.guest}</code> {share.read_only && <span className="chip">read-only</span>}
              </td>
              <td className="muted">{share.host}</td>
              <td className="right">
                <button className="link" onClick={() => act(() => api.removeShare(sandbox.name, share.guest))}>
                  remove
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {sandbox.backend === "krun" ? (
        <form
          className="row"
          onSubmit={(event) => {
            event.preventDefault();
            act(() => api.addShare(sandbox.name, host.trim(), guest.trim(), readOnly)).then(() => (setHost(""), setGuest("")));
          }}
        >
          <div className="grow">
            <FolderField label="Folder on this computer" required placeholder="/home/me/data" value={host} onChange={setHost} />
          </div>
          <label>
            Path in the sandbox
            <input required placeholder="/mnt/data" value={guest} onChange={(e) => setGuest(e.target.value)} />
          </label>
          <label className="check" title="Guards against accidents. A program with root rights in the sandbox can undo it.">
            <input type="checkbox" checked={readOnly} onChange={(e) => setReadOnly(e.target.checked)} />
            read-only
          </label>
          <button>Share folder</button>
        </form>
      ) : (
        <p className="muted">The qemu backend cannot share folders yet.</p>
      )}
      {sandbox.running && <p className="muted">Folder changes apply the next time the sandbox starts.</p>}
    </section>
  );
}

/** The first thing shown for a sandbox: is it running, how busy is it, what did the network do. */
function Status(props: { sandbox: Sandbox; recipes: Recipe[]; start: () => void; setTab: (tab: Tab) => void }) {
  const { sandbox, recipes } = props;
  const stats = useStats()[sandbox.name];
  const installed = sandbox.recipes.map((id) => recipes.find((r) => r.id === recipeName(id))?.name ?? recipeName(id));
  if (!sandbox.running) {
    return (
      <section className="centered">
        <h3>This sandbox is stopped</h3>
        <p className="muted">Start it to see its load and network activity here. Opening a terminal starts it too.</p>
        <div className="actions">
          <button className="primary" onClick={props.start}>
            Start {sandbox.name}
          </button>
        </div>
      </section>
    );
  }
  return (
    <>
      <div className="kpis">
        <Tile label="Connections allowed" value={`${stats?.allowed ?? 0}`} note="since it started" />
        <Tile label="Connections blocked" value={`${stats?.blocked ?? 0}`} note="since it started">
          {(stats?.blocked ?? 0) > 0 && (
            <button className="link" onClick={() => props.setTab("network")}>
              Show in the network log
            </button>
          )}
        </Tile>
        <Tile label="Network mode" value={sandbox.mode === "blacklist" ? "Blacklist" : "Whitelist"} note={`${sandbox.rules.length} rules`} />
        <Tile label="Software" value={`${installed.length}`} note={installed.slice(0, 4).join(", ") || "nothing installed yet"} />
      </div>
      <section className="vm">
        <h3>Load</h3>
        <VmLoad sandbox={sandbox} samples={stats?.samples ?? []} />
      </section>
      <Addresses sandbox={sandbox} />
    </>
  );
}

/** Where the sandbox is in its own network, and how this computer reaches it. */
function Addresses(props: { sandbox: Sandbox }) {
  const { sandbox } = props;
  const inside = sandbox.addresses;
  return (
    <section>
      <h3>Addresses</h3>
      <table>
        <tbody>
          {inside && (
            <>
              <tr>
                <td>Sandbox</td>
                <td>
                  <code>{inside.sandbox}</code>
                </td>
                <td className="muted">its own address, inside its private network</td>
              </tr>
              <tr>
                <td>This computer</td>
                <td>
                  <code>{inside.host}</code>
                </td>
                <td className="muted">as seen from the sandbox; needs a rule such as allow {inside.host}:8080</td>
              </tr>
              <tr>
                <td>Name server</td>
                <td>
                  <code>{inside.dns}</code>
                </td>
                <td className="muted">answers only for allowed names</td>
              </tr>
            </>
          )}
          {sandbox.ssh_port && (
            <tr>
              <td>SSH</td>
              <td>
                <code>127.0.0.1:{sandbox.ssh_port}</code>
              </td>
              <td className="muted">
                from this computer. Run <code>boxquell ssh-config --install</code> once, then <code>ssh boxquell-{sandbox.name}</code> works in a terminal, VS
                Code Remote-SSH and JetBrains Gateway.
              </td>
            </tr>
          )}
          {sandbox.ports.map((p) => (
            <tr key={p.host}>
              <td>Port {p.guest}</td>
              <td>
                <a href={`http://${p.bind ?? "127.0.0.1"}:${p.host}`} target="_blank" rel="noreferrer">
                  {p.bind ?? "127.0.0.1"}:{p.host}
                </a>
              </td>
              <td className="muted">forwarded from this computer</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="muted">The sandbox has no address in your network. Other devices reach it only through port forwards.</p>
    </section>
  );
}

/** What runs inside the sandbox from the moment it boots. */
function Autostart(props: { sandbox: Sandbox; recipes: Recipe[]; act: Act }) {
  const { sandbox, recipes, act } = props;
  const [command, setCommand] = useState("");
  const current = sandbox.autostart ?? [];
  const save = (next: string[]) => act(() => api.resize(sandbox.name, { autostart: next.length > 0 ? next : ["none"] }));
  // Installed software that has a start command, by the short name used on the command line.
  const agents = sandbox.recipes
    .map((id) => recipes.find((r) => r.id === recipeName(id)))
    .filter((r) => r?.command)
    .map((r) => ({ short: r!.id.split(":").pop()!, name: r!.name }));
  return (
    <section>
      <h3>Start with the sandbox</h3>
      <p className="muted">
        Runs inside the sandbox every time it boots, in the project root. Meant for servers such as Orca or code-server; an agent that needs a terminal is
        better opened from the Terminal tab.
      </p>
      {agents.map((agent) => (
        <label key={agent.short} className="check setting">
          <input
            type="checkbox"
            className="switch"
            checked={current.includes(agent.short)}
            onChange={() => save(current.includes(agent.short) ? current.filter((entry) => entry !== agent.short) : [...current, agent.short])}
          />
          {agent.name}
        </label>
      ))}
      <ul className="rules">
        {current
          .filter((entry) => !agents.some((agent) => agent.short === entry))
          .map((entry) => (
            <li key={entry}>
              <code>{entry}</code>
              <button className="link" onClick={() => save(current.filter((other) => other !== entry))}>
                remove
              </button>
            </li>
          ))}
      </ul>
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          save([...current, command.trim()]).then(() => setCommand(""));
        }}
      >
        <label className="grow">
          Another command
          <input required placeholder="npm run dev" value={command} onChange={(e) => setCommand(e.target.value)} />
        </label>
        <button>Add</button>
      </form>
      {sandbox.running && <p className="muted">Applies the next time the sandbox starts. Output goes to /var/log/boxquell-autostart-*.log in the sandbox.</p>}
    </section>
  );
}
