import { useEffect, useMemo, useState } from "react";
import { api, type Sandbox } from "./api";
import { StackedBars, timeline } from "./Usage";

type Act = (action: () => Promise<unknown>) => Promise<void>;

const SERIES = [
  { key: "up", label: "Sent", css: "series-input" },
  { key: "down", label: "Received", css: "series-output" },
] as const;

export function bytes(n: number): string {
  if (n < 1024) return `${Math.round(n)} B`;
  if (n < 1024 ** 2) return `${(n / 1024).toFixed(n < 10240 ? 1 : 0)} KiB`;
  if (n < 1024 ** 3) return `${(n / 1024 ** 2).toFixed(n < 10 * 1024 ** 2 ? 1 : 0)} MiB`;
  return `${(n / 1024 ** 3).toFixed(2)} GiB`;
}

/** What a sandbox sent and received over time, where it went, and how fast it may go. */
export function Traffic(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [days, setDays] = useState(1);
  const [report, setReport] = useState<Awaited<ReturnType<typeof api.traffic>> | null>(null);
  useEffect(() => {
    let alive = true;
    const load = () => api.traffic(sandbox.name, days).then((loaded) => alive && setReport(loaded), () => {});
    load();
    const timer = setInterval(load, 30_000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [sandbox.name, days]);
  const data = useMemo(() => {
    if (!report) return [];
    const all = timeline(report.bucket_ms, days, (t) => ({ t, up: 0, down: 0 }));
    for (const row of report.buckets) {
      const bucket = all.get(row.t);
      if (bucket) Object.assign(bucket, { up: row.up, down: row.down });
    }
    return [...all.values()];
  }, [report, days]);
  const total = data.reduce((sum, bucket) => ({ up: sum.up + bucket.up, down: sum.down + bucket.down }), { up: 0, down: 0 });
  const range = days === 1 ? "the last 24 hours" : `the last ${days} days`;
  return (
    <section>
      <div className="row between">
        <h3>Traffic</h3>
        <select value={days} onChange={(e) => setDays(Number(e.target.value))} aria-label="Time range">
          <option value={1}>Last 24 hours</option>
          <option value={7}>Last 7 days</option>
          <option value={30}>Last 30 days</option>
        </select>
      </div>
      <p className="muted">
        What this sandbox sent to and received from destinations outside in {range}: <strong>{bytes(total.up)}</strong> sent, <strong>{bytes(total.down)}</strong>{" "}
        received. Terminals and the project folder are not part of it.
      </p>
      {total.up + total.down > 0 && (
        <StackedBars buckets={data} hourly={days <= 2} series={SERIES} values={(bucket, key) => bucket[key as "up" | "down"]} format={bytes} label="Traffic" />
      )}
      {report && report.top.length > 0 && (
        <table className="sums">
          <thead>
            <tr>
              <th>Destination</th>
              <th className="right">Sent</th>
              <th className="right">Received</th>
              <th className="right">Connections</th>
            </tr>
          </thead>
          <tbody>
            {report.top.map((row) => (
              <tr key={row.host ?? "unknown"}>
                <td>{row.host ?? "unknown"}</td>
                <td className="right">{bytes(row.up)}</td>
                <td className="right">{bytes(row.down)}</td>
                <td className="right">{row.connections}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <SpeedLimits sandbox={sandbox} act={act} />
    </section>
  );
}

/** How fast the sandbox may send and receive. */
function SpeedLimits(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [up, setUp] = useState<string | null>(null);
  const [down, setDown] = useState<string | null>(null);
  const field = (label: string, limit: number | null | undefined, draft: string | null, set: (value: string | null) => void, key: "upload_kbit" | "download_kbit") => (
    <label>
      {label}
      <input
        type="number"
        min={0}
        step="0.5"
        placeholder="no limit"
        value={draft ?? (limit ? String(limit / 1000) : "")}
        onChange={(e) => set(e.target.value)}
        onBlur={() => {
          const kbit = Math.round(Number(draft || 0) * 1000);
          if (draft !== null && kbit !== (limit ?? 0)) act(() => api.resize(sandbox.name, { [key]: kbit })).then(() => set(null));
          else set(null);
        }}
      />
    </label>
  );
  return (
    <>
      <h3>Speed limit</h3>
      <p className="muted">How fast this sandbox may send and receive, in megabits per second. It applies at once, also to connections that are open.</p>
      <div className="row">
        {field("Sending, Mbit/s", sandbox.upload_kbit, up, setUp, "upload_kbit")}
        {field("Receiving, Mbit/s", sandbox.download_kbit, down, setDown, "download_kbit")}
      </div>
    </>
  );
}
