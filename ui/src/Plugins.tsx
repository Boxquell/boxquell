import { useEffect, useState, type FormEvent } from "react";
import { api, type Repo } from "./api";

/** Recipe repositories: git repositories that add agents, languages and tools. */
export function Plugins(props: { onChange: () => void }) {
  const [repos, setRepos] = useState<Repo[]>([]);
  const [url, setUrl] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => api.repos().then(setRepos, (err) => setError(String(err.message ?? err)));
  useEffect(() => {
    load();
  }, []);

  async function run(label: string, action: () => Promise<unknown>) {
    setBusy(label);
    setError(null);
    try {
      await action();
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
    }
    setBusy(null);
    await load();
    props.onChange();
  }

  const submit = (event: FormEvent) => {
    event.preventDefault();
    run("Adding…", () => api.addRepo(url.trim())).then(() => setUrl(""));
  };

  return (
    <div className="pane">
        <p className="muted">
          Agents, languages and tools come from plugin repositories: git repositories with recipe files. The official ones are added automatically. Recipes of other repositories run their own install scripts inside the sandbox and open the network
          destinations they list, so add only repositories you trust.
        </p>
        <table>
          <tbody>
            {repos.map((r) => (
              <tr key={r.name}>
                <td>
                  <strong>{r.name}</strong> {r.official && <span className="chip on">official</span>}
                </td>
                <td className="muted">{r.recipes} recipes</td>
                <td className="muted">
                  {r.url} @ {r.revision}
                </td>
                <td className="right">
                  <button className="link" disabled={!!busy} onClick={() => run("Updating…", () => api.updateRepo(r.name))}>
                    update
                  </button>
                  <button
                    className="link danger"
                    disabled={!!busy}
                    onClick={() => confirm(`Remove the repository ${r.name}? Software already installed from it stays.`) && run("Removing…", () => api.removeRepo(r.name))}
                  >
                    remove
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {error && <p className="error">{error}</p>}
        <form onSubmit={submit}>
          <label>
            Git URL of a repository
            <input required placeholder="https://example.com/someone/boxquell-recipes.git" value={url} onChange={(e) => setUrl(e.target.value)} />
          </label>
          <div className="actions">
            <button className="primary" disabled={!!busy}>
              {busy ?? "Add repository"}
            </button>
          </div>
        </form>
    </div>
  );
}
