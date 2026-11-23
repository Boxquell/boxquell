import { CircleCheck } from "lucide-react";
import { BrandIcon } from "./BrandIcon";
import { useEffect, useState, type FormEvent } from "react";
import { api, type Recipe, type System } from "./api";
import { FolderField } from "./FolderPicker";
import { BackendChoice, MachineControls } from "./Machine";
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
  const system = systems.find((candidate) => candidate.id === os);
  const [building, setBuilding] = useState<string | null>(null);
  const family = systems.find((system) => system.id === os)?.family ?? "debian";
  // Only software whose install script is written for the chosen system.
  const fitting = props.recipes.filter((recipe) => (recipe.os && recipe.os.length > 0 ? recipe.os : ["debian"]).includes(family));
  const chosen = picked.filter((entry) => fitting.some((recipe) => recipe.id === entry.split("@")[0]));
  const nameOk = NAME.test(name);
  const last = step === STEPS.length - 1;

  /** Build the chosen system's image if it does not exist yet, and wait for it. */
  async function ensureSystem() {
    if (!system || system.built) return;
    await api.buildSystem(os);
    for (;;) {
      await new Promise((resolve) => setTimeout(resolve, 1500));
      const now = (await api.systems()).find((candidate) => candidate.id === os);
      setSystems((list) => list.map((candidate) => (candidate.id === os && now ? now : candidate)));
      const lines = (now?.job?.log ?? "").trim().split("\n");
      setBuilding(lines[lines.length - 1] || "starting…");
      if (now?.built && now.job?.state !== "running") return;
      if (now?.job?.state === "failed") throw new Error(`Building ${system.name} failed: ${lines.slice(-3).join(" ")}`);
    }
  }

  async function create() {
    setBusy(true);
    setError(null);
    try {
      await ensureSystem();
      setBuilding(null);
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
          <FolderField label="Project folder on this computer" placeholder="/home/me/project" value={workspace} onChange={setWorkspace} />
          <p className="muted">The folder appears in the sandbox as /workspace; nothing else of your files is visible there. Leave it empty for a sandbox without your files.</p>
          <div className="shelf">
            <h3>Operating system</h3>
            <div className="os-tiles" role="radiogroup" aria-label="Operating system">
              {/* The default system first, then what is ready, then the rest. */}
              {[...systems]
                .sort((a, b) => Number(b.id === "debian") - Number(a.id === "debian") || Number(b.built) - Number(a.built))
                .map((candidate) => (
                <button
                  key={candidate.id}
                  type="button"
                  role="radio"
                  aria-checked={candidate.id === os}
                  className={`os-tile ${candidate.id === os ? "picked" : ""}`}
                  onClick={() => setOs(candidate.id)}
                >
                  {candidate.id === os && <CircleCheck className="os-check" size={18} aria-hidden />}
                  <span className="os-art" aria-hidden="true">
                    <BrandIcon svg={candidate.icon} size={34}>
                      {candidate.name.slice(0, 2)}
                    </BrandIcon>
                  </span>
                  <strong>{candidate.name}</strong>
                  <small>{candidate.built ? "Ready to use" : "Set up on first use"}</small>
                </button>
              ))}
            </div>
          </div>
          {system && <p className="muted">{system.description}</p>}
          {system && !system.built && <p className="muted">Its image is built once, the first time it is used. That needs Docker on this computer and takes a few minutes.</p>}
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
          <MachineControls
            value={{ cpus, ram, disk }}
            onChange={(next) => {
              setCpus(next.cpus);
              setRam(next.ram);
              setDisk(next.disk);
            }}
          />
          <h3>How it runs</h3>
          <BackendChoice value={backend} onChange={setBackend} />
          <p className="muted">CPUs, memory and disk can be changed later.</p>
        </section>
      )}

      {last && (
        <section>
          <h3>Review</h3>
          <table>
            <tbody>
              <Row label="Name" value={name} onEdit={() => setStep(0)} />
              <Row label="Project folder" value={workspace.trim() || "none"} onEdit={() => setStep(0)} />
              <Row label="System" value={`${system?.name ?? os}${system && !system.built ? " (will be built first)" : ""}`} onEdit={() => setStep(0)} />
              <Row label="Software" value={chosen.map((entry) => entry.split(":").pop()).join(", ") || "nothing yet"} onEdit={() => setStep(1)} />
              <Row label="Network" value={`${mode}${hosts.length > 0 ? `, also allowing ${hosts.join(", ")}` : ""}`} onEdit={() => setStep(2)} />
              <Row label="Machine" value={`${cpus} CPUs, ${ram} MiB memory, ${disk} GiB disk, ${backend}`} onEdit={() => setStep(3)} />
            </tbody>
          </table>
          {chosen.length > 0 && <p className="muted">The sandbox is created and started, then the software installs in the background. You can watch it in the Software tab.</p>}
        </section>
      )}

      {building && (
        <p className="warning" role="status">
          <strong>Building {system?.name}…</strong> {building}
        </p>
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
          {last ? (busy ? (building ? "Building the system…" : "Creating…") : chosen.length > 0 ? "Create and install" : "Create sandbox") : "Continue"}
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
