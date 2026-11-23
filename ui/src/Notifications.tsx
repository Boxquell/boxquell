import { BellOff, Check, X } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { api, type NetEvent, type Settings } from "./api";

/** A destination a sandbox tried to reach and was refused. */
export interface Blocked {
  key: string;
  sandbox: string;
  /** Host name when known, otherwise the address. */
  target: string;
  /** Only names can be allowed with one click; addresses need a rule with a port. */
  host: string | null;
  port: number | null;
  reason: string;
  count: number;
  last: number;
}

function matches(pattern: string, host: string): boolean {
  if (pattern === "*") return true;
  if (pattern.startsWith("*.")) return host.endsWith(pattern.slice(1)) && host.length > pattern.length - 1;
  return pattern === host;
}

function add(list: Blocked[], event: NetEvent): Blocked[] {
  // A lockdown blocks everything on purpose; that is not news.
  if (event.allowed || event.reason === "network lockdown") return list;
  const target = event.host ?? event.ip ?? "unknown";
  const key = `${event.sandbox} ${target}`;
  const entry = list.find((item) => item.key === key);
  const next: Blocked = {
    key,
    sandbox: event.sandbox,
    target,
    host: event.host,
    port: event.kind === "dns" ? (entry?.port ?? null) : event.port,
    reason: event.reason ?? "",
    count: (entry?.count ?? 0) + 1,
    last: Math.max(entry?.last ?? 0, event.ts_ms),
  };
  return [next, ...list.filter((item) => item.key !== key)].sort((a, b) => b.last - a.last).slice(0, 200);
}

/**
 * The blocked destinations of all sandboxes: recent ones from the log, then
 * live. Dismissed and allowed entries stay away until the destination is
 * blocked again; the daemon remembers them, so they do not return when the
 * page is opened anew.
 */
export function useBlocked(settings: Settings) {
  const [list, setList] = useState<Blocked[]>([]);
  const [handled, setHandled] = useState<Record<string, number>>({});
  useEffect(() => {
    let alive = true;
    api.handledNotifications().then((known) => alive && setHandled(known), () => {});
    api.blockedEvents().then((events) => {
      if (alive) setList((current) => events.sort((a, b) => a.seq - b.seq).reduce(add, current));
    }, () => {});
    const stop = api.followBlocked((event) => setList((current) => add(current, event)));
    return () => {
      alive = false;
      stop();
    };
  }, []);
  const dismiss = useCallback((key: string) => {
    const until = Date.now();
    setHandled((known) => ({ ...known, [key]: until }));
    api.markHandled([{ key, until }]).then(setHandled, () => {});
  }, []);
  const shown = list.filter((item) => item.last > (handled[item.key] ?? 0) && !settings.ignore_blocked.some((pattern) => matches(pattern, item.target)));
  return { blocked: shown, dismiss };
}

type Act = (action: () => Promise<unknown>) => Promise<void>;

export function Notifications(props: {
  blocked: Blocked[];
  dismiss: (key: string) => void;
  settings: Settings;
  saveSettings: (settings: Settings) => void;
  act: Act;
  open: (sandbox: string) => void;
}) {
  const { blocked, dismiss, settings, saveSettings, act } = props;
  const allow = (item: Blocked) => act(() => api.addRule(item.sandbox, `allow ${item.host}`)).then(() => dismiss(item.key));
  const ignore = (item: Blocked) => saveSettings({ ...settings, ignore_blocked: [...settings.ignore_blocked, item.target] });

  return (
    <>
      <section>
        <h3>
          <span className="grow">Blocked connections</span>
          {blocked.length > 1 && <button onClick={() => blocked.forEach((item) => dismiss(item.key))}>Dismiss all</button>}
        </h3>
        {blocked.length === 0 ? (
          <p className="muted">Nothing is waiting. When a sandbox tries to reach something its rules do not allow, it shows up here.</p>
        ) : (
          <ul className="notices">
            {blocked.map((item) => (
              <li key={item.key}>
                <div className="grow">
                  <strong>
                    {item.target}
                    {item.port ? `:${item.port}` : ""}
                  </strong>
                  <small className="muted">
                    <button className="link" onClick={() => props.open(item.sandbox)}>
                      {item.sandbox}
                    </button>
                    {" · "}
                    {item.count === 1 ? "once" : `${item.count} times`}, last at {new Date(item.last).toLocaleTimeString()} · {item.reason}
                  </small>
                </div>
                {item.host ? (
                  <button className="primary" onClick={() => allow(item)} title={`Add the rule "allow ${item.host}" to ${item.sandbox}`}>
                    <Check size={15} aria-hidden="true" /> Allow
                  </button>
                ) : (
                  <button onClick={() => props.open(item.sandbox)} title="An address needs a rule of its own, with a port if you like">
                    Add a rule…
                  </button>
                )}
                <button onClick={() => ignore(item)} title="Do not show or announce this destination again; it stays blocked">
                  <BellOff size={15} aria-hidden="true" /> Ignore
                </button>
                <button className="icon" onClick={() => dismiss(item.key)} title="Dismiss" aria-label={`Dismiss ${item.target}`}>
                  <X size={15} />
                </button>
              </li>
            ))}
          </ul>
        )}
        <p className="muted">Allowing adds a rule to that sandbox. Ignoring keeps the destination blocked and stops telling you about it. Everything stays in each sandbox's log.</p>
      </section>

      {settings.ignore_blocked.length > 0 && (
        <section>
          <h3>Ignored destinations</h3>
          <p className="muted">Blocked without notice, in every sandbox.</p>
          <ul className="rules">
            {settings.ignore_blocked.map((pattern) => (
              <li key={pattern}>
                <code>{pattern}</code>
                <button className="link" onClick={() => saveSettings({ ...settings, ignore_blocked: settings.ignore_blocked.filter((other) => other !== pattern) })}>
                  notify again
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}
    </>
  );
}
