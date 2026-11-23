import { useEffect, useMemo, useState } from "react";
import { api, type Price, type UsageReport, type UsageRow } from "./api";
import { Tile } from "./Overview";

type Act = (action: () => Promise<unknown>) => Promise<void>;

/** The four kinds of tokens, always in this order and always in these colours. */
const SERIES = [
  { key: "input", label: "Input", css: "series-input" },
  { key: "output", label: "Output", css: "series-output" },
  { key: "cache_read", label: "Read from cache", css: "series-cache_read" },
  { key: "cache_write", label: "Written to cache", css: "series-cache_write" },
] as const;

export function count(n: number): string {
  if (n >= 1e9) return `${(n / 1e9).toFixed(2)} B`;
  if (n >= 1e6) return `${(n / 1e6).toFixed(n >= 1e7 ? 1 : 2)} M`;
  if (n >= 1e3) return `${(n / 1e3).toFixed(n >= 1e4 ? 0 : 1)} k`;
  return String(Math.round(n));
}

export function dollars(n: number): string {
  return n >= 100 ? `$${n.toFixed(0)}` : n >= 0.995 ? `$${n.toFixed(2)}` : n > 0 ? `$${n.toFixed(n < 0.01 ? 4 : 3)}` : "$0";
}

interface Bucket {
  t: number;
  input: number;
  output: number;
  cache_read: number;
  cache_write: number;
  cost: number;
  requests: number;
}

/** Every stretch of time in the range, also the empty ones, with what was used in it. */
function buckets(rows: UsageRow[], bucketMs: number, days: number): Bucket[] {
  const end = Math.floor(Date.now() / bucketMs) * bucketMs;
  const start = Math.floor((Date.now() - days * 86_400_000) / bucketMs) * bucketMs + bucketMs;
  const all = new Map<number, Bucket>();
  for (let t = start; t <= end; t += bucketMs) all.set(t, { t, input: 0, output: 0, cache_read: 0, cache_write: 0, cost: 0, requests: 0 });
  for (const row of rows) {
    const bucket = all.get(row.t);
    if (!bucket) continue;
    for (const { key } of SERIES) bucket[key] += row[key];
    bucket.cost += row.cost ?? 0;
    bucket.requests += row.requests;
  }
  return [...all.values()];
}

/** One kind of value in a chart of stacked bars; `css` names its colour class. */
export interface Series {
  key: string;
  label: string;
  css: string;
}

/**
 * Stacked bars per hour or day, with the figures of a bar on hover. `values`
 * gives a bar's amount per series; `format` writes an amount, `foot` the last
 * line of the hover box.
 */
export function StackedBars<B extends { t: number }>(props: {
  buckets: B[];
  hourly: boolean;
  series: readonly Series[];
  values: (bucket: B, key: string) => number;
  format: (value: number) => string;
  foot?: (bucket: B) => React.ReactNode;
  label: string;
}) {
  const { buckets: data, hourly, series } = props;
  const [hover, setHover] = useState<number | null>(null);
  const [width, height, left, bottom, top] = [860, 240, 52, 22, 8];
  const total = (b: B) => series.reduce((sum, { key }) => sum + props.values(b, key), 0);
  const peak = Math.max(1, ...data.map(total));
  // A round number above the peak, so the axis reads in clean steps.
  const magnitude = 10 ** Math.floor(Math.log10(peak));
  const ceiling = [1, 2, 2.5, 5, 10].map((step) => step * magnitude).find((step) => step >= peak) ?? peak;
  const plot = { width: width - left, height: height - bottom - top };
  const slot = plot.width / Math.max(1, data.length);
  const bar = Math.max(2, Math.min(28, slot - 2));
  const y = (value: number) => top + plot.height * (1 - value / ceiling);
  const label = (t: number) =>
    hourly ? new Date(t).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" }) : new Date(t).toLocaleDateString([], { day: "numeric", month: "short" });
  const every = Math.max(1, Math.ceil(data.length / 8));
  const shown = hover === null ? null : data[hover];

  return (
    <div className="chart">
      <ul className="legend" aria-label="What the colours mean">
        {series.map(({ key, label, css }) => (
          <li key={key}>
            <span className={`swatch ${css}`} aria-hidden="true" /> {label}
          </li>
        ))}
      </ul>
      <svg viewBox={`0 0 ${width} ${height}`} role="img" aria-label={`${props.label} per ${hourly ? "hour" : "day"}; the table below lists the totals`} onMouseLeave={() => setHover(null)}>
        {[0, 0.5, 1].map((share) => (
          <g key={share}>
            <line className="grid" x1={left} x2={width} y1={y(ceiling * share)} y2={y(ceiling * share)} />
            <text className="axis" x={left - 8} y={y(ceiling * share) + 4} textAnchor="end">
              {props.format(ceiling * share)}
            </text>
          </g>
        ))}
        {data.map((bucket, index) => {
          const x = left + slot * index + (slot - bar) / 2;
          let stacked = 0;
          const kinds = series.filter(({ key }) => props.values(bucket, key) > 0);
          return (
            <g key={bucket.t}>
              {index % every === 0 && (
                <text className="axis" x={left + slot * index + slot / 2} y={height - 6} textAnchor="middle">
                  {label(bucket.t)}
                </text>
              )}
              {kinds.map(({ key, css }, at) => {
                const from = y(stacked);
                stacked += props.values(bucket, key);
                const to = y(stacked);
                // A gap of the surface colour between the parts; the top one gets round corners.
                const gap = at === 0 ? 0 : 2;
                const size = Math.max(0, from - to - gap);
                const last = at === kinds.length - 1;
                return <rect key={key} className={css} x={x} y={to} width={bar} height={size} rx={last ? Math.min(4, bar / 2, size) : 0} />;
              })}
              <rect className={hover === index ? "hit on" : "hit"} x={left + slot * index} y={top} width={slot} height={plot.height} onMouseEnter={() => setHover(index)} />
            </g>
          );
        })}
      </svg>
      {shown && hover !== null && (
        <div className="chart-tip" style={{ left: `${Math.min(88, Math.max(12, ((left + slot * (hover + 0.5)) / width) * 100))}%` }} role="status">
          <strong>{hourly ? `${new Date(shown.t).toLocaleDateString([], { day: "numeric", month: "short" })}, ${label(shown.t)}` : label(shown.t)}</strong>
          {series.map(({ key, label, css }) => (
            <span key={key}>
              <span className={`swatch ${css}`} aria-hidden="true" /> {label} <b>{props.format(props.values(shown, key))}</b>
            </span>
          ))}
          {props.foot && <span className="sum">{props.foot(shown)}</span>}
        </div>
      )}
    </div>
  );
}

/** Every stretch of time in the range, also the empty ones, as `make` fills it. */
export function timeline<B extends { t: number }>(bucketMs: number, days: number, make: (t: number) => B): Map<number, B> {
  const end = Math.floor(Date.now() / bucketMs) * bucketMs;
  const start = Math.floor((Date.now() - days * 86_400_000) / bucketMs) * bucketMs + bucketMs;
  const all = new Map<number, B>();
  for (let t = start; t <= end; t += bucketMs) all.set(t, make(t));
  return all;
}

function Sums(props: { title: string; first: string; rows: { name: string; tokens: number; cost: number; requests: number }[]; unpriced?: string[] }) {
  if (props.rows.length === 0) return null;
  return (
    <section>
      <h3>{props.title}</h3>
      <table className="sums">
        <thead>
          <tr>
            <th>{props.first}</th>
            <th className="right">Tokens</th>
            <th className="right">Requests</th>
            <th className="right">Estimated cost</th>
          </tr>
        </thead>
        <tbody>
          {props.rows.map((row) => (
            <tr key={row.name}>
              <td>{row.name || "unknown"}</td>
              <td className="right">{count(row.tokens)}</td>
              <td className="right">{row.requests}</td>
              <td className="right">{props.unpriced?.includes(row.name) ? <span className="muted">no price</span> : dollars(row.cost)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/** Limits per key and day; a key that reached one is refused until the next day. */
function Limits(props: { report: UsageReport; keys: string[]; act: Act; reload: () => void }) {
  const { report, act } = props;
  const [drafts, setDrafts] = useState<Record<string, { usd: string; tokens: string }>>({});
  const limit = (key: string) => report.limits.find((entry) => entry.key === key);
  const save = (key: string, usd: string, tokens: string) =>
    act(async () => {
      const all = await api.keyLimits();
      const next = { daily_usd: Number(usd) > 0 ? Number(usd) : undefined, daily_tokens: Number(tokens) > 0 ? Math.round(Number(tokens) * 1e6) : undefined };
      if (next.daily_usd === undefined && next.daily_tokens === undefined) delete all[key];
      else all[key] = next;
      await api.setKeyLimits(all);
      setDrafts((current) => ({ ...current, [key]: undefined as never }));
      props.reload();
    });
  if (props.keys.length === 0) return null;
  return (
    <section>
      <h3>Limits per key</h3>
      <p className="muted">
        A key that reaches its limit for the day is refused for the rest of it: the agent gets an answer that says so, and you get a notification. The day
        runs from midnight to midnight, UTC. Costs are estimates from the price list below.
      </p>
      <table className="sums">
        <thead>
          <tr>
            <th>Key</th>
            <th className="right">Today</th>
            <th>Dollars per day</th>
            <th>Million tokens per day</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {props.keys.map((key) => {
            const current = limit(key);
            const draft = drafts[key] ?? { usd: current?.daily_usd ? String(current.daily_usd) : "", tokens: current?.daily_tokens ? String(current.daily_tokens / 1e6) : "" };
            const edit = (change: Partial<{ usd: string; tokens: string }>) => setDrafts((all) => ({ ...all, [key]: { ...draft, ...change } }));
            return (
              <tr key={key}>
                <td>
                  {key} {current?.reached && <span className="chip blocked">limit reached</span>}
                </td>
                <td className="right">{current ? `${count(current.tokens_today)} · ${dollars(current.cost_today)}` : ""}</td>
                <td>
                  <input type="number" min={0} step="0.5" placeholder="no limit" value={draft.usd} onChange={(e) => edit({ usd: e.target.value })} aria-label={`Dollars per day for ${key}`} />
                </td>
                <td>
                  <input type="number" min={0} step="0.5" placeholder="no limit" value={draft.tokens} onChange={(e) => edit({ tokens: e.target.value })} aria-label={`Million tokens per day for ${key}`} />
                </td>
                <td className="right">
                  <button disabled={!drafts[key]} onClick={() => save(key, draft.usd, draft.tokens)}>
                    Save
                  </button>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </section>
  );
}

/** The price list behind the estimates: per million tokens, the first fitting pattern counts. */
function Prices(props: { act: Act; reload: () => void }) {
  const [prices, setPrices] = useState<Price[] | null>(null);
  const [changed, setChanged] = useState(false);
  useEffect(() => {
    api.prices().then(setPrices, () => {});
  }, []);
  if (!prices) return null;
  const edit = (index: number, change: Partial<Price>) => {
    setPrices(prices.map((price, at) => (at === index ? { ...price, ...change } : price)));
    setChanged(true);
  };
  const number = (index: number, key: "input" | "output" | "cache_read" | "cache_write") => (
    <td>
      <input type="number" min={0} step="0.01" value={prices[index][key]} onChange={(e) => edit(index, { [key]: Number(e.target.value) })} aria-label={`${key} price of ${prices[index].model}`} />
    </td>
  );
  return (
    <section>
      <details>
        <summary>
          <h3>Price list</h3>
        </summary>
        <p className="muted">
          US dollars per million tokens. A model's name is compared with the patterns from the top, <code>*</code> stands for anything, and the first that fits
          counts. The list that comes with Boxquell holds the providers' list prices of 23 November 2026; prices change, and discounts, batches and long
          prompts are priced differently, so read the costs as estimates.
        </p>
        <table className="sums prices">
          <thead>
            <tr>
              <th>Model</th>
              <th>Input</th>
              <th>Output</th>
              <th>Cache read</th>
              <th>Cache write</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {prices.map((price, index) => (
              <tr key={index}>
                <td>
                  <input spellCheck={false} value={price.model} onChange={(e) => edit(index, { model: e.target.value })} aria-label="Model pattern" />
                </td>
                {number(index, "input")}
                {number(index, "output")}
                {number(index, "cache_read")}
                {number(index, "cache_write")}
                <td>
                  <button
                    className="link"
                    onClick={() => {
                      setPrices(prices.filter((_, at) => at !== index));
                      setChanged(true);
                    }}
                  >
                    remove
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        <div className="actions left">
          <button
            onClick={() => {
              setPrices([{ model: "", input: 0, output: 0, cache_read: 0, cache_write: 0 }, ...prices]);
              setChanged(true);
            }}
          >
            Add a model
          </button>
          <button
            className="primary"
            disabled={!changed}
            onClick={() =>
              props.act(() =>
                api.setPrices(prices.filter((price) => price.model.trim())).then((saved) => {
                  setPrices(saved);
                  setChanged(false);
                  props.reload();
                }),
              )
            }
          >
            Save prices
          </button>
        </div>
      </details>
    </section>
  );
}

/** What the sandboxes used at model APIs: tokens over time, what that costs, and limits per key. */
export function Usage(props: { act: Act; secrets: string[] }) {
  const [days, setDays] = useState(7);
  const [report, setReport] = useState<UsageReport | null>(null);
  const [tick, setTick] = useState(0);
  useEffect(() => {
    let alive = true;
    const load = () => api.usage(days).then((loaded) => alive && setReport(loaded), () => {});
    load();
    const timer = setInterval(load, 15_000);
    return () => {
      alive = false;
      clearInterval(timer);
    };
  }, [days, tick]);
  const hourly = days <= 2;
  const data = useMemo(() => (report ? buckets(report.rows, report.bucket_ms, days) : []), [report, days]);
  if (!report) return null;
  const total = data.reduce((sum, bucket) => ({ tokens: sum.tokens + SERIES.reduce((n, { key }) => n + bucket[key], 0), cost: sum.cost + bucket.cost, requests: sum.requests + bucket.requests, cached: sum.cached + bucket.cache_read }), {
    tokens: 0,
    cost: 0,
    requests: 0,
    cached: 0,
  });
  const keys = [...new Set([...report.by_key.map((row) => row.name), ...report.limits.map((entry) => entry.key), ...props.secrets])].filter(Boolean);
  const range = days === 1 ? "the last 24 hours" : `the last ${days} days`;
  return (
    <>
      <header className="hero">
        <div className="grow">
          <h2>Usage</h2>
          <p className="muted">Tokens your sandboxes used at model APIs, read from the answers as they pass through Boxquell.</p>
        </div>
        <select value={days} onChange={(e) => setDays(Number(e.target.value))} aria-label="Time range">
          <option value={1}>Last 24 hours</option>
          <option value={7}>Last 7 days</option>
          <option value={30}>Last 30 days</option>
          <option value={90}>Last 90 days</option>
        </select>
      </header>
      <div className="kpis">
        <Tile label="Tokens" value={count(total.tokens)} note={`in ${range}`} />
        <Tile label="Estimated cost" value={dollars(total.cost)} note={report.unpriced.length > 0 ? `without ${report.unpriced.length} model${report.unpriced.length > 1 ? "s" : ""} that have no price` : "at the price list below"} />
        <Tile label="Requests" value={String(total.requests)} note="answers that reported their tokens" />
        <Tile label="From the cache" value={total.tokens > 0 ? `${Math.round((100 * total.cached) / total.tokens)} %` : "0 %"} note="of all tokens, billed at a lower price" />
      </div>
      <section>
        <h3>Tokens per {hourly ? "hour" : "day"}</h3>
        {total.tokens === 0 ? (
          <p className="muted">
            Nothing was used in {range}. Usage shows up here when an agent in a sandbox talks to a model API with a key that Boxquell keeps for it; requests
            that sign in another way pass through unseen.
          </p>
        ) : (
          <StackedBars
            buckets={data}
            hourly={hourly}
            series={SERIES}
            values={(bucket, key) => bucket[key as (typeof SERIES)[number]["key"]]}
            format={count}
            label="Tokens used"
            foot={(bucket) => (
              <>
                {bucket.requests} requests <b>{dollars(bucket.cost)}</b>
              </>
            )}
          />
        )}
      </section>
      <Sums title="By key" first="Key" rows={report.by_key} />
      <Sums title="By sandbox" first="Sandbox" rows={report.by_sandbox} />
      <Sums title="By model" first="Model" rows={report.by_model} unpriced={report.unpriced} />
      <Limits report={report} keys={keys} act={props.act} reload={() => setTick((n) => n + 1)} />
      <Prices act={props.act} reload={() => setTick((n) => n + 1)} />
    </>
  );
}
