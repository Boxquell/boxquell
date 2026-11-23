import { useEffect, useRef, useState } from "react";
import { api, type NetEvent } from "./api";

interface Toast {
  key: string;
  sandbox: string;
  target: string;
  host: string | null;
  reason: string;
}

const SHOWN_FOR_MS = 9000;
const QUIET_FOR_MS = 60_000;

/** A message for each blocked connection, with a button to allow the destination. */
export function Toasts(props: { enabled: boolean; ignored: string[]; onOpen: (sandbox: string) => void; onChange: () => void }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const seen = useRef(new Map<string, number>());

  useEffect(() => {
    if (!props.enabled) {
      setToasts([]);
      return;
    }
    return api.followBlocked((event: NetEvent) => {
      // A lockdown blocks everything; one message per connection would flood the page.
      if (event.reason === "network lockdown") return;
      const target = event.host ?? event.ip ?? "unknown";
      // Destinations the user chose not to hear about.
      if (props.ignored.some((pattern) => pattern === target || pattern === "*" || (pattern.startsWith("*.") && target.endsWith(pattern.slice(1))))) return;
      const key = `${event.sandbox} ${target}`;
      const now = Date.now();
      if (now - (seen.current.get(key) ?? 0) < QUIET_FOR_MS) return;
      seen.current.set(key, now);
      const port = event.port && event.kind !== "dns" ? `:${event.port}` : "";
      setToasts((list) => [...list.slice(-3), { key, sandbox: event.sandbox, target: target + port, host: event.host, reason: event.reason ?? "" }]);
      setTimeout(() => setToasts((list) => list.filter((t) => t.key !== key)), SHOWN_FOR_MS);
    });
  }, [props.enabled, props.ignored.join(" ")]);

  const dismiss = (key: string) => setToasts((list) => list.filter((t) => t.key !== key));
  return (
    <div className="toasts" role="status" aria-live="polite">
      {toasts.map((t) => (
        <div key={t.key} className="toast">
          <div className="grow">
            <strong>Blocked in {t.sandbox}</strong>
            <div>{t.target}</div>
            <small className="muted">{t.reason}</small>
          </div>
          <div className="toast-actions">
            {t.host && (
              <button
                className="primary"
                onClick={() => {
                  api.addRule(t.sandbox, `allow ${t.host}`).then(props.onChange, () => {});
                  dismiss(t.key);
                }}
              >
                Allow
              </button>
            )}
            <button
              onClick={() => {
                props.onOpen(t.sandbox);
                dismiss(t.key);
              }}
            >
              Show
            </button>
            <button className="link" aria-label="Dismiss" onClick={() => dismiss(t.key)}>
              ×
            </button>
          </div>
        </div>
      ))}
    </div>
  );
}
