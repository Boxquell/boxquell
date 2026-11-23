import { useState, type FormEvent } from "react";
import { api, type Secret } from "./api";

const PRESETS = ["anthropic", "claude-oauth", "openai", "openrouter", "github"];

export function Secrets(props: { secrets: Secret[]; act: (action: () => Promise<unknown>) => Promise<void> }) {
  const [name, setName] = useState("anthropic");
  const [custom, setCustom] = useState("");
  const [hosts, setHosts] = useState("");
  const [env, setEnv] = useState("");
  const [value, setValue] = useState("");
  const isCustom = name === "custom";

  async function submit(event: FormEvent) {
    event.preventDefault();
    const list = (text: string) => text.split(/[\s,]+/).filter(Boolean);
    await props.act(() => (isCustom ? api.setSecret(custom, value, list(hosts), list(env)) : api.setSecret(name, value)));
    setValue("");
  }

  return (
    <div className="pane">
        <p className="muted">
          Values stay on this computer. A sandbox only sees a placeholder; the real value is put into requests to the listed hosts on their way out.
        </p>
        <table>
          <tbody>
            {props.secrets.map((s) => (
              <tr key={s.name}>
                <td>
                  <strong>{s.name}</strong>
                </td>
                <td className="muted">{s.hosts.join(", ")}</td>
                <td className="muted">{s.env.join(", ")}</td>
                <td className="right">
                  <button className="link danger" onClick={() => confirm(`Delete the secret ${s.name}?`) && props.act(() => api.removeSecret(s.name))}>
                    delete
                  </button>
                </td>
              </tr>
            ))}
            {props.secrets.length === 0 && (
              <tr>
                <td className="muted">No secrets stored.</td>
              </tr>
            )}
          </tbody>
        </table>
        <form onSubmit={submit}>
          <div className="row">
            <label>
              Service
              <select value={name} onChange={(e) => setName(e.target.value)}>
                {PRESETS.map((p) => (
                  <option key={p}>{p}</option>
                ))}
                <option value="custom">other…</option>
              </select>
            </label>
            <label className="grow">
              Value
              <input type="password" required autoComplete="off" value={value} onChange={(e) => setValue(e.target.value)} />
            </label>
          </div>
          {isCustom && (
            <div className="row">
              <label>
                Name
                <input required pattern="[a-z0-9][a-z0-9-]*" value={custom} onChange={(e) => setCustom(e.target.value)} />
              </label>
              <label className="grow">
                Hosts that may receive it
                <input required placeholder="api.example.com" value={hosts} onChange={(e) => setHosts(e.target.value)} />
              </label>
              <label>
                Variable in the sandbox
                <input required placeholder="EXAMPLE_API_KEY" value={env} onChange={(e) => setEnv(e.target.value)} />
              </label>
            </div>
          )}
          <div className="actions">
            <button className="primary">Save secret</button>
          </div>
        </form>
    </div>
  );
}
