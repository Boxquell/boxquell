import { useEffect, useState } from "react";
import { api, type Settings } from "./api";

export function Preferences(props: { settings: Settings; onChange: (settings: Settings) => void }) {
  const { settings } = props;
  const [addresses, setAddresses] = useState<{ address: string; interface: string }[]>([]);
  useEffect(() => {
    api.addresses().then(setAddresses, () => {});
  }, []);
  const toggle = (key: "notify_blocked" | "toast_blocked") => props.onChange({ ...settings, [key]: !settings[key] });
  const toggleAddress = (address: string) =>
    props.onChange({ ...settings, listen: settings.listen.includes(address) ? settings.listen.filter((a) => a !== address) : [...settings.listen, address] });
  // Addresses that were chosen earlier but are not present right now stay visible.
  const known = [...addresses.filter((a, i) => addresses.findIndex((x) => x.address === a.address) === i), ...settings.listen.filter((a) => !addresses.some((x) => x.address === a)).map((address) => ({ address, interface: "not connected" }))];

  return (
    <div className="pane">
        <h3>When a connection is blocked</h3>
        <label className="check setting">
          <input type="checkbox" className="switch" checked={settings.notify_blocked} onChange={() => toggle("notify_blocked")} />
          Show a notification on this computer
        </label>
        <label className="check setting">
          <input type="checkbox" className="switch" checked={settings.toast_blocked} onChange={() => toggle("toast_blocked")} />
          Show a message on this page, with a button to allow the destination
        </label>
        <p className="muted">Each destination is announced at most once a minute. Every attempt is in the network log either way.</p>

        <h3>Where this page can be opened</h3>
        <label className="check setting">
          <input type="checkbox" className="switch" checked disabled />
          This computer only (127.0.0.1), always on
        </label>
        {known.map((a) => (
          <label key={a.address} className="check setting">
            <input type="checkbox" className="switch" checked={settings.listen.includes(a.address)} onChange={() => toggleAddress(a.address)} />
            <span>
              <code>{a.address}</code> <span className="muted">{a.interface}</span>
            </span>
          </label>
        ))}
        <div className="row">
          <label>
            Port on these addresses
            <input type="number" min={1024} max={65535} value={settings.listen_port} onChange={(e) => props.onChange({ ...settings, listen_port: +e.target.value })} />
          </label>
        </div>
        {settings.listen.length > 0 && (
          <div className="warning">
            <div>
              <strong>The connection is not encrypted.</strong> Anyone in that network who learns the address with its token can control every sandbox,
              including their terminals. Choose only networks you trust, such as a VPN.
            </div>
            <div>
              Open it elsewhere with the token from <code>boxquell ui --no-open</code>, replacing the address:{" "}
              <code>
                http://{settings.listen[0]}:{settings.listen_port}/#token=…
              </code>
            </div>
          </div>
        )}
        <p className="muted">Changes to the addresses apply after the daemon restarts (boxquell daemon --stop).</p>
    </div>
  );
}
