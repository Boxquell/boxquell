import { Camera, FileCode2, History, LayoutTemplate } from "lucide-react";
import { useState } from "react";
import { api, type Sandbox } from "./api";

type Act = (action: () => Promise<unknown>) => Promise<void>;

/** Saved states of a sandbox's disk to go back to, and saving the sandbox as a template for new ones. */
export function Snapshots(props: { sandbox: Sandbox; act: Act }) {
  const { sandbox, act } = props;
  const [label, setLabel] = useState("");
  const [template, setTemplate] = useState("");
  const [saved, setSaved] = useState<string | null>(null);
  const [wrote, setWrote] = useState<string | null>(null);
  const snapshots = [...(sandbox.snapshots ?? [])].reverse();
  const pause = sandbox.running ? " The sandbox stops for a moment and starts again." : "";
  return (
    <section>
      <h3>
        <History size={16} aria-hidden /> Snapshots
      </h3>
      <p className="muted">
        A snapshot keeps the sandbox's disk as it is now: installed software, settings, everything outside the project folder. Going back to one undoes what
        happened in the sandbox since. The project folder on this computer is not part of it.{pause}
      </p>
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          act(() => api.takeSnapshot(sandbox.name, label)).then(() => setLabel(""));
        }}
      >
        <label className="grow">
          What to call it
          <input placeholder="before the big refactoring" value={label} onChange={(e) => setLabel(e.target.value)} />
        </label>
        <button className="primary">
          <Camera size={15} aria-hidden /> Take snapshot
        </button>
      </form>
      {snapshots.length > 0 && (
        <ul className="notices plain">
          {snapshots.map((snapshot) => (
            <li key={snapshot.id}>
              <span className="grow plain">
                <strong>{snapshot.label || "Snapshot"}</strong>
                <small className="muted">
                  {new Date(snapshot.created_ms).toLocaleString()} · {snapshot.recipes.length} pieces of software
                </small>
              </span>
              <button
                onClick={() =>
                  confirm(`Go back to "${snapshot.label || snapshot.id}"? Everything that happened in ${sandbox.name} since then is lost.`) &&
                  act(() => api.restoreSnapshot(sandbox.name, snapshot.id))
                }
              >
                Go back to it
              </button>
              <button className="link bad" onClick={() => confirm("Delete this snapshot?") && act(() => api.deleteSnapshot(sandbox.name, snapshot.id))}>
                delete
              </button>
            </li>
          ))}
        </ul>
      )}
      <h3>
        <LayoutTemplate size={16} aria-hidden /> Template
      </h3>
      <p className="muted">
        Save this sandbox as a starting point: new sandboxes made from the template begin with its system and software already installed.{pause}
      </p>
      <form
        className="row"
        onSubmit={(event) => {
          event.preventDefault();
          const name = template.trim();
          act(() => api.saveTemplate(sandbox.name, name, `Made from ${sandbox.name}`)).then(() => {
            setSaved(name);
            setTemplate("");
          });
        }}
      >
        <label className="grow">
          Name of the template
          <input required pattern="[a-z0-9][a-z0-9-]*" placeholder="node-project" value={template} onChange={(e) => setTemplate(e.target.value.toLowerCase())} />
        </label>
        <button>Save as template</button>
      </form>
      {saved && <p className="muted">Saved. Choose "{saved}" under "Start from" when you create a sandbox.</p>}
      {sandbox.workspace && (
        <>
          <h3>
            <FileCode2 size={16} aria-hidden /> Project file
          </h3>
          <p className="muted">
            Write this sandbox's description (system, software, rules, ports, variables) as <code>boxquell.toml</code> into the project folder. Anyone with the
            project can then create the same sandbox: with <code>boxquell up</code> in the folder, or by choosing the folder when creating a sandbox. Keys are
            never written into it.
          </p>
          <div className="actions left">
            <button
              onClick={() =>
                act(() =>
                  api.writeProjectFile(sandbox.name, false).then(
                    (written) => setWrote(written.file),
                    (err) => {
                      if (!String(err.message ?? err).includes("exists already") || !confirm("The folder has a boxquell.toml already. Replace it?")) throw err;
                      return api.writeProjectFile(sandbox.name, true).then((written) => setWrote(written.file));
                    },
                  ),
                )
              }
            >
              Write boxquell.toml
            </button>
            {wrote && <span className="muted">Wrote {wrote}</span>}
          </div>
        </>
      )}
    </section>
  );
}
