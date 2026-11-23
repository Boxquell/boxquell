import { useEffect, useState } from "react";
import { Cpu, Play, Plus, Square, SquareTerminal } from "lucide-react";
import { BrandIcon, brandColors } from "./BrandIcon";
import { api, type Recipe, type Sandbox, type Sample, type Stats } from "./api";

function bytes(n: number): string {
  if (n < 1024) return `${Math.round(n)} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} kB`;
  if (n < 1024 ** 3) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(1)} GB`;
}

const clock = (ms: number) => new Date(ms).toLocaleTimeString();

/** A single measure over the last minutes: 2px line, soft area, a value per point on hover. */
function Sparkline(props: { samples: Sample[]; value: (s: Sample) => number; max?: number; label: (s: Sample) => string }) {
  const { samples } = props;
  const [width, height] = [220, 44];
  if (samples.length < 2) return <svg className="spark" viewBox={`0 0 ${width} ${height}`} aria-hidden="true" />;
  const values = samples.map(props.value);
  const top = Math.max(props.max ?? 0, ...values, 1);
  const x = (i: number) => (i / (samples.length - 1)) * width;
  const y = (v: number) => height - 2 - (v / top) * (height - 6);
  const line = values.map((v, i) => `${i === 0 ? "M" : "L"}${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(" ");
  const step = width / (samples.length - 1);
  return (
    <svg className="spark" viewBox={`0 0 ${width} ${height}`} preserveAspectRatio="none" role="img" aria-label="Trend over the last minutes">
      <path d={`${line} L${width},${height} L0,${height} Z`} className="spark-area" />
      <path d={line} className="spark-line" />
      {samples.map((s, i) => (
        <rect key={s.ts_ms} x={x(i) - step / 2} y={0} width={step} height={height} className="spark-hit">
          <title>{`${clock(s.ts_ms)}  ${props.label(s)}`}</title>
        </rect>
      ))}
    </svg>
  );
}

/** A ratio against its limit. The figure is always shown, so the colour never carries the meaning alone. */
function Meter(props: { label: string; used: number; total: number; figure: string }) {
  const share = props.total > 0 ? Math.min(1, props.used / props.total) : 0;
  const level = share > 0.9 ? "critical" : share > 0.75 ? "warning" : "";
  return (
    <div className="meter-row">
      <span>{props.label}</span>
      <div className={`meter ${level === "critical" ? "level-full" : level === "warning" ? "level-high" : ""}`} role="meter" aria-label={props.label} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(share * 100)}>
        <div style={{ width: `${share * 100}%` }} />
      </div>
      <span className="figure">
        {props.figure}
        {level && <small> {level === "critical" ? "almost full" : "high"}</small>}
      </span>
    </div>
  );
}

function Tile(props: { label: string; value: string; note?: string; children?: React.ReactNode }) {
  return (
    <div className="tile">
      <span className="tile-label">{props.label}</span>
      <span className="tile-value">{props.value}</span>
      {props.note && <small className="muted">{props.note}</small>}
      {props.children}
    </div>
  );
}

/** CPU, network, memory and disk of one running sandbox. */
export function VmLoad(props: { sandbox: Sandbox; samples: Sample[] }) {
  const { sandbox: s, samples } = props;
  const now = samples.at(-1);
  if (!now) return <p className="muted">{s.paused ? "Paused: no measurements while the sandbox is frozen." : "Waiting for the first measurements…"}</p>;
  return (
    <div className="vm-body">
      <Tile label="CPU" value={`${Math.round(now.cpu)} %`} note={`of ${s.cpus} CPUs`}>
        <Sparkline samples={samples} value={(x) => x.cpu} max={100} label={(x) => `${Math.round(x.cpu)} %`} />
      </Tile>
      <Tile label="Network" value={`${bytes(now.rx_rate + now.tx_rate)}/s`} note={`↓ ${bytes(now.rx_rate)}/s  ↑ ${bytes(now.tx_rate)}/s`}>
        <Sparkline samples={samples} value={(x) => x.rx_rate + x.tx_rate} label={(x) => `${bytes(x.rx_rate + x.tx_rate)}/s`} />
      </Tile>
      <div className="tile meters">
        <Meter label="Memory" used={now.mem_used_mib} total={now.mem_total_mib} figure={`${now.mem_used_mib} / ${now.mem_total_mib} MiB`} />
        <Meter label="Disk" used={now.disk_used_gib} total={now.disk_total_gib} figure={`${now.disk_used_gib.toFixed(1)} / ${now.disk_total_gib.toFixed(0)} GiB`} />
      </div>
    </div>
  );
}

/** Polls the load and connection counts of all running sandboxes. */
export function useStats(): Stats {
  const [stats, setStats] = useState<Stats>({});
  useEffect(() => {
    const load = () => api.stats().then(setStats, () => {});
    load();
    const timer = setInterval(load, 2000);
    return () => clearInterval(timer);
  }, []);
  return stats;
}

/** How busy this computer itself is, sandboxes included. */
function HostLoad() {
  const [host, setHost] = useState<{ cpus: number; samples: Sample[] } | null>(null);
  useEffect(() => {
    const load = () => api.hostStats().then(setHost, () => {});
    load();
    const timer = setInterval(load, 2000);
    return () => clearInterval(timer);
  }, []);
  // An older daemon does not know this question; then there is nothing to show.
  const now = host?.samples?.at(-1);
  if (!host || !now) return null;
  return (
    <section className="vm">
      <div className="vm-head">
        <Cpu size={16} aria-hidden />
        <strong className="grow">This computer</strong>
        <span className="muted">everything running on it, sandboxes included</span>
      </div>
      <div className="vm-body">
        <Tile label="CPU" value={`${Math.round(now.cpu)} %`} note={`of ${host.cpus} CPUs`}>
          <Sparkline samples={host.samples} value={(x) => x.cpu} max={100} label={(x) => `${Math.round(x.cpu)} %`} />
        </Tile>
        <Tile label="Memory" value={`${(now.mem_used_mib / 1024).toFixed(1)} GiB`} note={`of ${(now.mem_total_mib / 1024).toFixed(1)} GiB in use`}>
          <Sparkline samples={host.samples} value={(x) => x.mem_used_mib} max={now.mem_total_mib} label={(x) => `${(x.mem_used_mib / 1024).toFixed(1)} GiB`} />
        </Tile>
        <div className="tile meters">
          <Meter label="Memory" used={now.mem_used_mib} total={now.mem_total_mib} figure={`${Math.round((100 * now.mem_used_mib) / Math.max(1, now.mem_total_mib))} %`} />
          <Meter label="Disk" used={now.disk_used_gib} total={now.disk_total_gib} figure={`${now.disk_used_gib.toFixed(0)} / ${now.disk_total_gib.toFixed(0)} GiB`} />
        </div>
      </div>
    </section>
  );
}

export { Tile };

export function Overview(props: {
  sandboxes: Sandbox[];
  recipes: Recipe[];
  open: (name: string) => void;
  openTerminal: (name: string) => void;
  create: () => void;
  act: (action: () => Promise<unknown>) => Promise<void>;
}) {
  const stats = useStats();
  const running = props.sandboxes.filter((s) => s.running);
  const all = Object.values(stats);
  const last = (name: string) => stats[name]?.samples.at(-1);
  const traffic = running.reduce((sum, s) => sum + (last(s.name)?.rx_rate ?? 0) + (last(s.name)?.tx_rate ?? 0), 0);
  // Running ones first, then by name.
  const ordered = [...props.sandboxes].sort((a, b) => Number(b.running) - Number(a.running) || a.name.localeCompare(b.name));

  if (props.sandboxes.length === 0) {
    return (
      <section className="welcome">
        <img src="/icon.svg" alt="" width="64" height="64" />
        <h2>Welcome to Boxquell</h2>
        <p className="muted readable">
          A sandbox is a small computer of its own for one project. A coding agent works inside it; what it may reach on the network is up to you, and your
          API keys stay on this computer.
        </p>
        <ol className="steps-list">
          <li>
            <strong>Create a sandbox</strong>
            <span className="muted">Give it a name and, if you like, a project folder.</span>
          </li>
          <li>
            <strong>Add an agent and its key</strong>
            <span className="muted">Claude Code, Codex and others install with a click.</span>
          </li>
          <li>
            <strong>Open a terminal or give a task</strong>
            <span className="muted">Work with the agent, or let it work in the background.</span>
          </li>
        </ol>
        <button className="primary large" onClick={props.create}>
          <Plus size={17} aria-hidden /> Create your first sandbox
        </button>
      </section>
    );
  }

  return (
    <>
      <header className="hero">
        <div className="grow">
          <h2>Home</h2>
          <p className="muted">
            {running.length === 0 ? "No sandbox is running." : `${running.length} of ${props.sandboxes.length} sandboxes running`}
            {running.length > 0 && <> · {bytes(traffic)}/s</>}
            {all.reduce((n, s) => n + s.blocked, 0) > 0 && <span className="chip blocked">{all.reduce((n, s) => n + s.blocked, 0)} blocked</span>}
          </p>
        </div>
        <button className="primary" onClick={props.create}>
          <Plus size={16} aria-hidden /> New sandbox
        </button>
      </header>

      <div className="box-grid">
        {ordered.map((s) => {
          const now = last(s.name);
          const software = s.recipes.map((id) => props.recipes.find((r) => r.id === id.split("@")[0])).filter((r): r is Recipe => !!r);
          const agents = software.filter((r) => r.category === "agent");
          return (
            <article key={s.name} className={s.running ? "box-card on" : "box-card"}>
              <button className="box-open" onClick={() => props.open(s.name)} aria-label={`Open ${s.name}`}>
                <span className="box-mark" style={s.os_icon ? brandColors(s.os_icon_color) : undefined} aria-hidden="true">
                  <BrandIcon svg={s.os_icon} size={24}>
                    {s.name.slice(0, 2)}
                  </BrandIcon>
                  <span className={s.paused ? "dot paused" : s.running ? "dot on" : "dot"} />
                </span>
                <span className="grow">
                  <strong>{s.name}</strong>
                  <small>
                    {s.job?.state === "running" ? "installing…" : s.paused ? "paused" : s.running ? "running" : "stopped"} · {s.os_name ?? s.os ?? "debian"}
                  </small>
                </span>
              </button>
              <div className="box-tags">
                {agents.map((agent) => (
                  <span key={agent.id} className="chip">
                    <BrandIcon svg={agent.icon} size={12} /> {agent.name}
                  </span>
                ))}
                {software.length > agents.length && <span className="chip">+{software.length - agents.length} more</span>}
                {software.length === 0 && <span className="chip">no software yet</span>}
                {s.warnings.length > 0 && (
                  <span className="chip paused" title={s.warnings.join("\n")}>
                    needs attention
                  </span>
                )}
                {(stats[s.name]?.blocked ?? 0) > 0 && <span className="chip blocked">{stats[s.name].blocked} blocked</span>}
              </div>
              {s.running && now ? (
                <div className="box-stats">
                  <span>
                    <b>{Math.round(now.cpu)} %</b> CPU
                  </span>
                  <span>
                    <b>{(now.mem_used_mib / 1024).toFixed(1)} GiB</b> memory
                  </span>
                  <span>
                    <b>{bytes(now.rx_rate + now.tx_rate)}/s</b> network
                  </span>
                </div>
              ) : (
                <div className="box-stats muted">
                  <span>
                    {s.cpus} CPUs · {Math.round(s.ram_mib / 102.4) / 10} GiB · {s.disk_gib} GiB disk
                  </span>
                </div>
              )}
              <div className="box-actions">
                <button onClick={() => props.openTerminal(s.name)}>
                  <SquareTerminal size={15} aria-hidden /> Terminal
                </button>
                {s.running ? (
                  <button onClick={() => props.act(() => api.stop(s.name))}>
                    <Square size={13} aria-hidden /> Stop
                  </button>
                ) : (
                  <button className="primary" onClick={() => props.act(() => api.start(s.name))}>
                    <Play size={14} aria-hidden /> Start
                  </button>
                )}
              </div>
            </article>
          );
        })}
        <button className="box-card new" onClick={props.create}>
          <Plus size={22} aria-hidden />
          New sandbox
        </button>
      </div>

      <HostLoad />
    </>
  );
}
