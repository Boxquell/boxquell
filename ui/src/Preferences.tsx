import type { Settings } from "./api";

export function Preferences(props: { settings: Settings; onChange: (settings: Settings) => void; onClose: () => void }) {
  const { settings } = props;
  const toggle = (key: keyof Settings) => props.onChange({ ...settings, [key]: !settings[key] });
  return (
    <div className="backdrop" onMouseDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <div className="dialog">
        <h2>Settings</h2>
        <h3>When a connection is blocked</h3>
        <label className="check">
          <input type="checkbox" checked={settings.notify_blocked} onChange={() => toggle("notify_blocked")} />
          Show a notification on this computer
        </label>
        <label className="check">
          <input type="checkbox" checked={settings.toast_blocked} onChange={() => toggle("toast_blocked")} />
          Show a message on this page, with a button to allow the destination
        </label>
        <p className="muted">Each destination is announced at most once a minute. Every attempt is in the network log either way.</p>
        <div className="actions">
          <button className="primary" onClick={props.onClose}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
