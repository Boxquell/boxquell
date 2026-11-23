import { Search } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { api, type NetEvent, type Sandbox } from "./api";

type Act = (action: () => Promise<unknown>) => Promise<void>;

/** Close events carry the byte counts of the connection opened under the same id. */
function merge(rows: NetEvent[], event: NetEvent): NetEvent[] {
  if (rows.some((r) => r.seq === event.seq)) return rows;
  if (event.phase === "close") {
    const at = rows.findIndex((r) => r.id === event.id && r.kind === event.kind && r.phase === "open");
    if (at < 0) return rows;
    const copy = rows.slice();
    copy[at] = { ...copy[at], bytes_up: event.bytes_up, bytes_down: event.bytes_down, duration_ms: event.duration_ms };
    return copy;
  }
  return [event, ...rows].slice(0, 2000);
}

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} kB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

const RESULTS = ["all", "allowed", "blocked"] as const;
const KINDS = ["all", "dns", "tcp", "http", "udp"] as const;

/** Every connection attempt of a sandbox, live, with filters and a search. */
export function NetworkLog(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [rows, setRows] = useState<NetEvent[]>([]);
  const [result, setResult] = useState<(typeof RESULTS)[number]>("all");
  const [kind, setKind] = useState<(typeof KINDS)[number]>("all");
  const [query, setQuery] = useState("");
  const [paused, setPaused] = useState(false);

  useEffect(() => {
    let alive = true;
    api.events(sandbox.name, 2000).then((events) => {
      if (alive) setRows((current) => events.sort((a, b) => a.seq - b.seq).reduce(merge, current));
    });
    if (paused) return () => void (alive = false);
    const stop = api.follow(sandbox.name, (event) => setRows((current) => merge(current, event)));
    return () => {
      alive = false;
      stop();
    };
  }, [sandbox.name, paused]);

  const shown = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return rows.filter((e) => {
      if (result !== "all" && (result === "allowed") !== e.allowed) return false;
      if (kind !== "all" && e.kind !== kind) return false;
      if (!needle) return true;
      return [e.host, e.ip, e.port, e.reason, e.method, e.path, e.status, e.secret].some((field) => field != null && String(field).toLowerCase().includes(needle));
    });
  }, [rows, result, kind, query]);
  const blocked = rows.filter((e) => !e.allowed).length;

  return (
    <section>
      <div className="log-bar">
        <label className="search grow">
          <Search size={15} aria-hidden="true" />
          <input type="search" placeholder="Search host, address, port, rule, path…" value={query} onChange={(e) => setQuery(e.target.value)} aria-label="Search the network log" />
        </label>
        <div className="tabbar small" role="group" aria-label="Result">
          {RESULTS.map((value) => (
            <button key={value} className={result === value ? "active" : ""} onClick={() => setResult(value)}>
              {value}
              {value === "blocked" && blocked > 0 && <small>{blocked}</small>}
            </button>
          ))}
        </div>
        <select value={kind} onChange={(e) => setKind(e.target.value as (typeof KINDS)[number])} aria-label="Type">
          {KINDS.map((value) => (
            <option key={value} value={value}>
              {value === "all" ? "all types" : value}
            </option>
          ))}
        </select>
        <button onClick={() => setPaused(!paused)}>{paused ? "Resume" : "Hold"}</button>
      </div>
      <p className="muted">
        {shown.length} of {rows.length} entries{paused ? ", held: new entries are not added" : ", live"}.
      </p>
      <div className="log tall">
        <table>
          <thead>
            <tr>
              <th>Time</th>
              <th>Result</th>
              <th>Type</th>
              <th>Destination</th>
              <th>Why</th>
              <th className="right">Sent / received</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {shown.slice(0, 500).map((e) => {
              const target = e.host ?? e.ip ?? "?";
              return (
                <tr key={e.seq} className={e.allowed ? "" : "blocked"}>
                  <td>{new Date(e.ts_ms).toLocaleTimeString()}</td>
                  <td>{e.allowed ? "allowed" : "blocked"}</td>
                  <td>{e.kind}</td>
                  <td>
                    {e.method ? `${e.method} ` : ""}
                    {target}
                    {e.port && e.kind !== "dns" ? `:${e.port}` : ""}
                    {e.path ?? ""}
                    {e.status ? ` → ${e.status}` : ""}
                    {e.secret ? ` (secret ${e.secret})` : ""}
                  </td>
                  <td className="muted">{e.reason}</td>
                  <td className="right">{e.kind !== "dns" && e.bytes_up != null ? `${size(e.bytes_up)} / ${size(e.bytes_down ?? 0)}` : ""}</td>
                  <td className="right">
                    {e.allowed && e.host && e.kind !== "dns" && (
                      <button className="link bad" title="Add a rule that blocks this destination" onClick={() => act(() => api.addRule(sandbox.name, `deny ${e.host}`))}>
                        Block
                      </button>
                    )}
                    {!e.allowed && e.host && (
                      <button className="link" onClick={() => act(() => api.addRule(sandbox.name, `allow ${e.host}`))}>
                        allow
                      </button>
                    )}
                  </td>
                </tr>
              );
            })}
            {shown.length === 0 && (
              <tr>
                <td colSpan={7} className="muted">
                  {rows.length === 0 ? "Nothing yet. Every connection attempt of this sandbox shows up here." : "Nothing matches the filters."}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {shown.length > 500 && <p className="muted">Showing the newest 500 matches. Narrow the search to see older ones.</p>}
    </section>
  );
}
