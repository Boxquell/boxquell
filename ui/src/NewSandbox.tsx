import { useEffect, useState, type FormEvent } from "react";
import { api, type Recipe, type System } from "./api";
import { RecipeCards } from "./RecipeCards";

const STEPS = ["Basics", "Software", "Network", "Machine", "Review"] as const;
const NAME = /^[a-z0-9][a-z0-9-]{0,31}$/;

/** Creating a sandbox, one decision at a time. Nothing is created before the last step. */
export function NewSandbox(props: { recipes: Recipe[]; onClose: () => void; onCreated: (name: string) => void }) {
  const [step, setStep] = useState(0);
  const [name, setName] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [os, setOs] = useState("debian");
  const [systems, setSystems] = useState<System[]>([]);
  const [picked, setPicked] = useState<string[]>([]);
  const [mode, setMode] = useState<"whitelist" | "blacklist">("whitelist");
  const [hosts, setHosts] = useState<string[]>([]);
  const [host, setHost] = useState("");
  const [cpus, setCpus] = useState(2);
  const [ram, setRam] = useState(4096);
  const [disk, setDisk] = useState(20);
  const [backend, setBackend] = useState("krun");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.systems().then(setSystems, () => {});
  }, []);
  const built = systems.filter((system) => system.built);
  const notBuilt = systems.filter((system) => !system.built);
  const family = systems.find((system) => system.id === os)?.family ?? "debian";
  // Only software whose install script is written for the chosen system.
  const fitting = props.recipes.filter((recipe) => (recipe.os && recipe.os.length > 0 ? recipe.os : ["debian"]).includes(family));
  const chosen = picked.filter((entry) => fitting.some((recipe) => recipe.id === entry.split("@")[0]));
  const nameOk = NAME.test(name);
  const last = step === STEPS.length - 1;

  async function create() {
    setBusy(true);
    setError(null);
    try {
      await api.create({
        name,
        workspace: workspace.trim() || null,
        cpus,
        ram_mib: ram,
        disk_gib: disk,
        backend,
        os,
        network: mode,
        rules: hosts.map((entry) => `allow ${entry}`),
      });
      if (chosen.length > 0) await api.install(name, chosen);
      props.onCreated(name);
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
      setBusy(false);
    }
  }

  const next = (event: FormEvent) => {
    event.preventDefault();
    if (last) create();
    else setStep(step + 1);
  };
  const addHost = () => {
    const entry = host.trim();
    if (entry && !hosts.includes(entry)) setHosts([...hosts, entry]);
    setHost("");
  };

  return (
    <form className="wizard" onSubmit={next}>
      <ol className="steps" aria-label="Steps">
        {STEPS.map((label, index) => (
          <li key={label} className={index === step ? "current" : index < step ? "done" : ""} aria-current={index === step ? "step" : undefined}>
            <button type="button" disabled={index > step && !nameOk} onClick={() => (index <= step || nameOk) && setStep(index)}>
              <span className="number">{index < step ? "✓" : index + 1}</span>
              {label}
            </button>
          </li>
        ))}
      </ol>

      {step === 0 && (
        <section>
          <h3>Name and project</h3>
          <label>
            Name
            <input autoFocus required pattern="[a-z0-9][a-z0-9-]{0,31}" title="Lowercase letters, digits and dashes" placeholder="my-project" value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <label>
            Project folder on this computer
            <input placeholder="/home/me/project" value={workspace} onChange={(e) => setWorkspace(e.target.value)} />
          </label>
          <p className="muted">The folder appears in the sandbox as /workspace; nothing else of your files is visible there. Leave it empty for a sandbox without your files.</p>
          <label>
            Operating system
            <select value={os} onChange={(e) => setOs(e.target.value)}>
              {built.map((system) => (
                <option key={system.id} value={system.id}>
                  {system.name}
                </option>
              ))}
              {built.length === 0 && <option value="debian">Debian</option>}
            </select>
          </label>
          {notBuilt.length > 0 && <p className="muted">More systems appear here once built: {notBuilt.map((system) => `boxquell os build ${system.id}`).join(", ")}.</p>}
        </section>
      )}

      {step === 1 && (
        <section className="software">
          {fitting.length > 0 ? (
            <RecipeCards recipes={fitting} installed={[]} picked={chosen} busy={false} onChange={setPicked} />
          ) : (
            <p className="muted">There are no recipes for this system yet. You can still install software by hand in the sandbox.</p>
          )}
          <p className="muted">Optional. Anything can be added later; what a recipe needs is installed with it.</p>
        </section>
      )}

      {step === 2 && (
        <section>
          <h3>Network</h3>
          <div className="choices">
            <label className={mode === "whitelist" ? "choice picked" : "choice"}>
              <input type="radio" name="mode" checked={mode === "whitelist"} onChange={() => setMode("whitelist")} />
              <span>
                <strong>Whitelist</strong>
                <small>Nothing gets out unless it is allowed. The software you chose brings the destinations it needs.</small>
              </span>
            </label>
            <label className={mode === "blacklist" ? "choice picked" : "choice"}>
              <input type="radio" name="mode" checked={mode === "blacklist"} onChange={() => setMode("blacklist")} />
              <span>
                <strong>Blacklist</strong>
                <small>Everything public is reachable unless it is denied. Your own network stays closed either way.</small>
              </span>
            </label>
          </div>
          <label>
            Also allow
            <span className="row">
              <input
                className="grow"
                placeholder="example.com, *.example.com, 192.0.2.7, 10.0.0.0/8, optionally :port"
                value={host}
                onChange={(e) => setHost(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    addHost();
                  }
                }}
              />
              <button type="button" onClick={addHost}>
                Add
              </button>
            </span>
          </label>
          <ul className="rules">
            {hosts.map((entry) => (
              <li key={entry}>
                <code>allow {entry}</code>
                <button type="button" className="link" onClick={() => setHosts(hosts.filter((other) => other !== entry))}>
                  remove
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      {step === 3 && (
        <section>
          <h3>Machine</h3>
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
              Backend
              <select value={backend} onChange={(e) => setBackend(e.target.value)}>
                <option value="krun">krun</option>
                <option value="qemu">qemu</option>
              </select>
            </label>
          </div>
          <p className="muted">All of this can be changed later; a disk can only grow. The disk takes space only as it fills.</p>
        </section>
      )}

      {last && (
        <section>
          <h3>Review</h3>
          <table>
            <tbody>
              <Row label="Name" value={name} onEdit={() => setStep(0)} />
              <Row label="Project folder" value={workspace.trim() || "none"} onEdit={() => setStep(0)} />
              <Row label="System" value={systems.find((system) => system.id === os)?.name ?? os} onEdit={() => setStep(0)} />
              <Row label="Software" value={chosen.map((entry) => entry.split(":").pop()).join(", ") || "nothing yet"} onEdit={() => setStep(1)} />
              <Row label="Network" value={`${mode}${hosts.length > 0 ? `, also allowing ${hosts.join(", ")}` : ""}`} onEdit={() => setStep(2)} />
              <Row label="Machine" value={`${cpus} CPUs, ${ram} MiB memory, ${disk} GiB disk, ${backend}`} onEdit={() => setStep(3)} />
            </tbody>
          </table>
          {chosen.length > 0 && <p className="muted">The sandbox is created and started, then the software installs in the background. You can watch it in the Software tab.</p>}
        </section>
      )}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="actions wizard-actions">
        <button type="button" onClick={props.onClose}>
          Cancel
        </button>
        <span className="grow" />
        {step > 0 && (
          <button type="button" onClick={() => setStep(step - 1)}>
            Back
          </button>
        )}
        <button className="primary" disabled={busy || !nameOk}>
          {last ? (busy ? "Creating…" : chosen.length > 0 ? "Create and install" : "Create sandbox") : "Continue"}
        </button>
      </div>
    </form>
  );
}

function Row(props: { label: string; value: string; onEdit: () => void }) {
  return (
    <tr>
      <td className="muted">{props.label}</td>
      <td>{props.value}</td>
      <td className="right">
        <button type="button" className="link" onClick={props.onEdit}>
          change
        </button>
      </td>
    </tr>
  );
}
