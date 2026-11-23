import { ArrowDown, ArrowUp, Gauge, ShieldOff } from "lucide-react";
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

/** Values a limit can take, as megabits per second; 0 stands for no limit and sits at the right end. */
const SPEEDS = [0.5, 1, 2, 5, 10, 25, 50, 100, 250, 500, 0];
/** The same for data per run, in MiB. */
const AMOUNTS = [50, 100, 250, 500, 1024, 2048, 5120, 10240, 25600, 51200, 0];

function megabits(value: number): string {
  return value === 0 ? "No limit" : `${value} Mbit/s`;
}

function amount(mib: number): string {
  return mib === 0 ? "No limit" : mib >= 1024 ? `${mib / 1024} GiB` : `${mib} MiB`;
}

/**
 * A limit chosen on a slider with a few sensible stops. It is saved when the
 * slider is let go, not while it moves.
 */
function LimitSlider(props: { icon: React.ReactNode; label: string; stops: number[]; value: number; format: (value: number) => string; used?: number; save: (value: number) => void }) {
  // A value set elsewhere that is not one of the stops goes to the nearest one below it.
  const index = (value: number) => {
    if (value === 0) return props.stops.length - 1;
    let at = 0;
    props.stops.forEach((stop, i) => {
      if (stop !== 0 && stop <= value) at = i;
    });
    return at;
  };
  const [draft, setDraft] = useState<number | null>(null);
  const at = draft ?? index(props.value);
  const shown = props.stops[at];
  const share = (at / (props.stops.length - 1)) * 100;
  const commit = () => {
    if (draft !== null && props.stops[draft] !== props.stops[index(props.value)]) props.save(props.stops[draft]);
    setDraft(null);
  };
  const full = shown !== 0 && props.used !== undefined ? Math.min(1, props.used / (shown * 1024 * 1024)) : 0;
  return (
    <label className="slider limit">
      <span className="slider-head">
        {props.icon}
        <span className="grow">{props.label}</span>
        <output className={shown === 0 ? "off" : ""}>{props.format(shown)}</output>
      </span>
      <input
        type="range"
        min={0}
        max={props.stops.length - 1}
        step={1}
        value={at}
        onChange={(e) => setDraft(+e.target.value)}
        onPointerUp={commit}
        onKeyUp={commit}
        onBlur={commit}
        style={{ "--share": `${share}%` } as React.CSSProperties}
        aria-valuetext={props.format(shown)}
      />
      {props.used !== undefined && (
        <small className="muted limit-used">
          <span className={`meter ${full > 0.9 ? "level-full" : full > 0.75 ? "level-high" : ""}`} aria-hidden="true">
            <span style={{ width: `${full * 100}%` }} />
          </span>
          {bytes(props.used)} so far{shown !== 0 ? ` of ${props.format(shown)}` : ""}
        </small>
      )}
    </label>
  );
}

/** How fast the sandbox may send and receive, and how much per run before its network is cut off. */
function SpeedLimits(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const traffic = sandbox.traffic;
  const set = (change: Parameters<typeof api.resize>[1]) => act(() => api.resize(sandbox.name, change));
  return (
    <>
      <h3>
        <Gauge size={16} aria-hidden /> Speed
      </h3>
      <p className="muted">How fast this sandbox may send and receive. A change applies at once, also to connections that are open.</p>
      <div className="limits">
        <LimitSlider icon={<ArrowUp size={16} aria-hidden />} label="Sending" stops={SPEEDS} value={(sandbox.upload_kbit ?? 0) / 1000} format={megabits} save={(value) => set({ upload_kbit: Math.round(value * 1000) })} />
        <LimitSlider icon={<ArrowDown size={16} aria-hidden />} label="Receiving" stops={SPEEDS} value={(sandbox.download_kbit ?? 0) / 1000} format={megabits} save={(value) => set({ download_kbit: Math.round(value * 1000) })} />
      </div>
      <h3>
        <ShieldOff size={16} aria-hidden /> Data per run
      </h3>
      <p className="muted">
        Once this sandbox has sent or received this much, its network is cut off until you let it continue. Counting starts when the sandbox starts.
      </p>
      {traffic?.cut && (
        <div className="warning">
          <span>A limit was reached: the network of this sandbox is cut off.</span>
          <button className="link" onClick={() => act(() => api.continueAfterLimit(sandbox.name))}>
            Let it continue and count anew
          </button>
        </div>
      )}
      <div className="limits">
        <LimitSlider
          icon={<ArrowUp size={16} aria-hidden />}
          label="Sent"
          stops={AMOUNTS}
          value={traffic?.upload_limit_mib ?? 0}
          format={amount}
          used={sandbox.running ? (traffic?.up ?? 0) : undefined}
          save={(value) => set({ upload_limit_mib: value })}
        />
        <LimitSlider
          icon={<ArrowDown size={16} aria-hidden />}
          label="Received"
          stops={AMOUNTS}
          value={traffic?.download_limit_mib ?? 0}
          format={amount}
          used={sandbox.running ? (traffic?.down ?? 0) : undefined}
          save={(value) => set({ download_limit_mib: value })}
        />
      </div>
    </>
  );
}
