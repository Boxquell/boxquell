import { useCallback, useEffect, useRef, useState } from "react";
import { api, token, type Recipe, type Sandbox, type Secret } from "./api";
import { NewSandbox } from "./NewSandbox";
import { SandboxView, type Tab } from "./SandboxView";
import { Plugins } from "./Plugins";
import { Secrets } from "./Secrets";
import { Terminal, type Session } from "./Terminal";

export function App() {
  const [sandboxes, setSandboxes] = useState<Sandbox[]>([]);
  const [recipes, setRecipes] = useState<Recipe[]>([]);
  const [secrets, setSecrets] = useState<Secret[]>([]);
  const [selected, setSelected] = useState<string | null>(null);
  const [dialog, setDialog] = useState<"new" | "secrets" | "plugins" | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Terminals live here so they stay connected while another sandbox is shown.
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSession, setActiveSession] = useState<Record<string, number>>({});
  const [tabs, setTabs] = useState<Record<string, Tab>>({});
  const nextSession = useRef(1);
  const openTerminal = useCallback((sandbox: string, title: string, cmd?: string) => {
    const id = nextSession.current++;
    setSessions((list) => [...list, { id, sandbox, title, cmd }]);
    setActiveSession((active) => ({ ...active, [sandbox]: id }));
    setTabs((all) => ({ ...all, [sandbox]: "terminal" }));
  }, []);
  const closeTerminal = useCallback((id: number) => setSessions((list) => list.filter((s) => s.id !== id)), []);

  const refresh = useCallback(async () => {
    try {
      const [list, stored] = await Promise.all([api.sandboxes(), api.secrets()]);
      setSandboxes(list);
      setSecrets(stored);
      setError(null);
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
    }
  }, []);

  useEffect(() => {
    if (!token) return;
    api.recipes().then(setRecipes, () => {});
    refresh();
    const timer = setInterval(refresh, 2000);
    return () => clearInterval(timer);
  }, [refresh]);

  /** Run an action, show its error, and reload the lists. */
  const act = useCallback(
    async (action: () => Promise<unknown>) => {
      try {
        await action();
        setError(null);
      } catch (err) {
        setError(String(err instanceof Error ? err.message : err));
      }
      await refresh();
    },
    [refresh],
  );

  if (!token) {
    return (
      <main className="empty">
        <h1>Boxquell</h1>
        <p>
          This page needs its access token. Open it with <code>boxquell ui</code>.
        </p>
      </main>
    );
  }

  const current = sandboxes.find((s) => s.name === selected) ?? sandboxes[0] ?? null;

  return (
    <div className="layout">
      <aside>
        <h1 className="brand">
          <img src="/icon.svg" alt="" width="28" height="28" />
          Boxquell
        </h1>
        <nav>
          {sandboxes.map((s) => (
            <div key={s.name} className={s.name === current?.name ? "item active" : "item"}>
              <button className="grow" onClick={() => setSelected(s.name)}>
                <span className={s.paused ? "dot paused" : s.running ? "dot on" : "dot"} />
                <span className="grow">
                  <strong>{s.name}</strong>
                  <small>{s.job?.state === "running" ? "installing…" : s.paused ? "paused" : agentsOf(s, recipes) || (s.running ? "running" : "stopped")}</small>
                </span>
              </button>
              <button
                className="power"
                title={s.running ? `Stop ${s.name}` : `Start ${s.name}`}
                aria-label={s.running ? `Stop ${s.name}` : `Start ${s.name}`}
                onClick={() => act(() => (s.running ? api.stop(s.name) : api.start(s.name)))}
              >
                {s.running ? "■" : "▶"}
              </button>
            </div>
          ))}
          {sandboxes.length === 0 && <p className="muted pad">No sandboxes yet.</p>}
        </nav>
        <button className="primary" onClick={() => setDialog("new")}>
          New sandbox
        </button>
        <button onClick={() => setDialog("plugins")}>Plugins</button>
        <button onClick={() => setDialog("secrets")}>Secrets ({secrets.length})</button>
      </aside>
      <main>
        {error && (
          <div className="error" role="alert">
            {error}
            <button className="link" onClick={() => setError(null)}>
              dismiss
            </button>
          </div>
        )}
        {current ? (
          <>
            <SandboxView
              key={current.name}
              sandbox={current}
              recipes={recipes}
              secrets={secrets}
              act={act}
              openTerminal={openTerminal}
              tab={tabs[current.name] ?? "terminal"}
              setTab={(tab) => setTabs((all) => ({ ...all, [current.name]: tab }))}
            />
            <section className={(tabs[current.name] ?? "terminal") === "terminal" && sessions.some((s) => s.sandbox === current.name) ? "" : "hidden"}>
              <div className="tabs">
                {sessions
                  .filter((s) => s.sandbox === current.name)
                  .map((s) => (
                    <span key={s.id} className={shown(sessions, activeSession, current.name) === s.id ? "tab active" : "tab"}>
                      <button className="link" onClick={() => setActiveSession((active) => ({ ...active, [current.name]: s.id }))}>
                        {s.title}
                      </button>
                      <button className="link" title="Close this terminal" aria-label={`Close ${s.title}`} onClick={() => closeTerminal(s.id)}>
                        ×
                      </button>
                    </span>
                  ))}
              </div>
              {sessions.map((s) => (
                <Terminal key={s.id} session={s} visible={shown(sessions, activeSession, current.name) === s.id} onExit={() => {}} />
              ))}
            </section>
          </>
        ) : (
          <div className="empty">
            <p>Create a sandbox to run a coding agent with an explicit network allowlist.</p>
          </div>
        )}
      </main>
      {dialog === "new" && (
        <NewSandbox
          recipes={recipes}
          onClose={() => setDialog(null)}
          onCreated={(name) => {
            setDialog(null);
            setSelected(name);
            refresh();
          }}
        />
      )}
      {dialog === "plugins" && (
        <Plugins
          recipes={recipes}
          onChange={() => api.recipes().then(setRecipes, () => {})}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === "secrets" && <Secrets secrets={secrets} act={act} onClose={() => setDialog(null)} />}
    </div>
  );
}

/** The terminal on screen for `sandbox`: the chosen one, or its newest. */
function shown(sessions: Session[], active: Record<string, number>, sandbox: string): number | undefined {
  const own = sessions.filter((s) => s.sandbox === sandbox);
  return own.find((s) => s.id === active[sandbox])?.id ?? own[own.length - 1]?.id;
}

function agentsOf(sandbox: Sandbox, recipes: Recipe[]): string {
  return sandbox.recipes
    .map((id) => recipes.find((r) => r.id === id.split("@")[0]))
    .filter((r) => r?.category === "agent")
    .map((r) => r!.name)
    .join(", ");
}
