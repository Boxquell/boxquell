import { useState, type FormEvent } from "react";
import { api, type Recipe } from "./api";
import { RecipePicker } from "./RecipePicker";

export function NewSandbox(props: { recipes: Recipe[]; onClose: () => void; onCreated: (name: string) => void }) {
  const [name, setName] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [cpus, setCpus] = useState(2);
  const [ram, setRam] = useState(4096);
  const [disk, setDisk] = useState(20);
  const [backend, setBackend] = useState("krun");
  const [mode, setMode] = useState("whitelist");
  const [picked, setPicked] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    try {
      await api.create({ name, workspace: workspace.trim() || null, cpus, ram_mib: ram, disk_gib: disk, backend, network: mode });
      if (picked.length > 0) await api.install(name, picked);
      props.onCreated(name);
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
      setBusy(false);
    }
  }

  return (
    <div className="backdrop" onMouseDown={(e) => e.target === e.currentTarget && props.onClose()}>
      <form className="dialog" onSubmit={submit}>
        <h2>New sandbox</h2>
        <label>
          Name
          <input autoFocus required pattern="[a-z0-9][a-z0-9-]*" title="lowercase letters, digits and dashes" value={name} onChange={(e) => setName(e.target.value)} />
        </label>
        <label>
          Project folder on this computer (shared as /workspace, optional)
          <input placeholder="/home/me/project" value={workspace} onChange={(e) => setWorkspace(e.target.value)} />
        </label>
        <RecipePicker recipes={props.recipes} installed={[]} picked={picked} onChange={setPicked} />
        <div className="row">
          <label>
            CPUs
            <input type="number" min={1} max={64} value={cpus} onChange={(e) => setCpus(+e.target.value)} />
          </label>
          <label>
            Memory (MiB)
            <input type="number" min={256} step={256} value={ram} onChange={(e) => setRam(+e.target.value)} />
          </label>
          <label>
            Disk (GiB)
            <input type="number" min={4} value={disk} onChange={(e) => setDisk(+e.target.value)} />
          </label>
          <label>
            Network
            <select value={mode} onChange={(e) => setMode(e.target.value)}>
              <option value="whitelist">whitelist</option>
              <option value="blacklist">blacklist</option>
            </select>
          </label>
          <label>
            Backend
            <select value={backend} onChange={(e) => setBackend(e.target.value)}>
              <option value="krun">krun</option>
              <option value="qemu">qemu</option>
            </select>
          </label>
        </div>
        {error && <p className="error">{error}</p>}
        <div className="actions">
          <button type="button" onClick={props.onClose}>
            Cancel
          </button>
          <button className="primary" disabled={busy}>
            {busy ? "Creating…" : picked.length > 0 ? "Create and install" : "Create"}
          </button>
        </div>
      </form>
    </div>
  );
}
