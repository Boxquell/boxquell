import { Fragment, useCallback, useEffect, useRef, useState } from "react";
import { SshKeys } from "./SshKeys";
import { Mcp } from "./Mcp";
import { Usage } from "./Usage";
import { API_LEVEL, api, token, type Recipe, type Sandbox, type Secret, type Settings, type UpdateInfo } from "./api";
import { Preferences } from "./Preferences";
import { Toasts } from "./Toasts";
import { Bell, ChartColumn, LayoutDashboard, Plus, Power, Settings as SettingsIcon, SquareTerminal, ShieldAlert } from "lucide-react";
import { About } from "./About";
import { AgentSettings } from "./AgentSettings";
import { LogViewer } from "./LogViewer";
import { NewSandbox } from "./NewSandbox";
import { Notifications, useBlocked } from "./Notifications";
import { Overview } from "./Overview";
import { GlobalRules, SandboxView, type Tab } from "./SandboxView";
import { Plugins } from "./Plugins";
import { Secrets } from "./Secrets";
import { Terminal, type Session } from "./Terminal";

export function App() {
  const [sandboxes, setSandboxes] = useState<Sandbox[]>([]);
  const [recipes, setRecipes] = useState<Recipe[]>([]);
  const [secrets, setSecrets] = useState<Secret[]>([]);
  // `?box=name&tab=network` opens a sandbox and tab directly, e.g. from a bookmark.
  const [selected, setSelected] = useState<string | null>(() => new URLSearchParams(location.search).get("box"));
  // `?box=` opens that sandbox; otherwise the overview is the start page.
  const [page, setPage] = useState<Page>(() => {
    const query = new URLSearchParams(location.search);
    const wanted = query.get("page") as Page | null;
    return query.get("box") ? "sandbox" : wanted && ["terminals", "notifications", "usage", "settings", "new"].includes(wanted) ? wanted : "overview";
  });
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("general");
  // The sidebar can be folded away; the choice is remembered in this browser.
  const [collapsed, setCollapsed] = useState(() => localStorage.getItem("boxquell-sidebar") === "collapsed");
  const toggleSidebar = () =>
    setCollapsed((now) => {
      localStorage.setItem("boxquell-sidebar", now ? "open" : "collapsed");
      return !now;
    });
  /** Terminals fill everything right of the sidebar. */
  const [fullscreen, setFullscreen] = useState(false);
  /** The terminal shown on the Terminals page. */
  const [focused, setFocused] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [lockdown, setLockdown] = useState(false);
  const [outdated, setOutdated] = useState(false);
  const [release, setRelease] = useState<UpdateInfo | null>(null);
  const [settings, setSettings] = useState<Settings>({ notify_blocked: true, toast_blocked: true, ignore_blocked: [], browser_ui: false, listen: [], listen_port: 7878, update_repos: true, check_updates: true, idle_stop_minutes: 0, mcp_read_only: false, mcp_network_rules: false, mcp_delete: false, mcp_review_apply: false });
  const { blocked, dismiss } = useBlocked(settings);
  const saveSettings = useCallback((next: Settings) => {
    setSettings(next);
    api.setSettings(next).then(setSettings, () => {});
  }, []);
  // Terminals live here so they stay connected while another sandbox is shown.
  const [sessions, setSessions] = useState<Session[]>([]);
  const [activeSession, setActiveSession] = useState<Record<string, number>>({});
  const [tabs, setTabs] = useState<Record<string, Tab>>(() => {
    const query = new URLSearchParams(location.search);
    const [box, tab] = [query.get("box"), query.get("tab")];
    // Values from the address bar: accept only what a sandbox name and a tab can be.
    const known: Tab[] = ["overview", "terminal", "tasks", "review", "software", "config", "agent", "network", "log", "secrets", "settings"];
    const valid = box !== null && /^[a-z0-9][a-z0-9-]*$/.test(box) && known.includes(tab as Tab);
    return valid ? { [box]: tab as Tab } : {};
  });
  const nextSession = useRef(1);
  const openTerminal = useCallback((sandbox: string, title: string, cmd?: string, stay?: boolean) => {
    const id = nextSession.current++;
    setSessions((list) => [...list, { id, sandbox, title, cmd }]);
    setActiveSession((active) => ({ ...active, [sandbox]: id }));
    setFocused(id);
    // Opened from a sandbox, its terminal tab comes to the front; `stay` keeps the current page.
    if (!stay) setTabs((all) => ({ ...all, [sandbox]: "terminal" }));
  }, []);
  const closeTerminal = useCallback((id: number) => setSessions((list) => list.filter((s) => s.id !== id)), []);

  const refresh = useCallback(async () => {
    try {
      const [list, stored, locked] = await Promise.all([api.sandboxes(), api.secrets(), api.lockdown().catch(() => ({ on: false }))]);
      setSandboxes(list);
      setLockdown(locked.on);
      setSecrets(stored);
      setError(null);
    } catch (err) {
      setError(String(err instanceof Error ? err.message : err));
    }
  }, []);

  useEffect(() => {
    if (!token) return;
    api.level().then((level) => setOutdated(level < API_LEVEL), () => {});
    const look = () => api.updateStatus().then(setRelease, () => {});
    look();
    const hourly = setInterval(look, 60 * 60 * 1000);
    // Repositories change behind the page: by the daily fetch, the command line or another window.
    const catalogue = setInterval(() => api.recipes().then(setRecipes, () => {}), 30 * 1000);
    api.recipes().then(setRecipes, () => {});
    api.settings().then(setSettings, () => {});
    refresh();
    const timer = setInterval(refresh, 2000);
    return () => {
      clearInterval(timer);
      clearInterval(hourly);
      clearInterval(catalogue);
    };
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

  const current = page === "sandbox" ? (sandboxes.find((s) => s.name === selected) ?? null) : null;
  const go = (next: Page, sandbox?: string) => {
    setPage(next);
    if (sandbox) setSelected(sandbox);
  };
  const sandboxTab = current ? (tabs[current.name] ?? "overview") : null;
  // Terminals stay mounted on every page; this decides which of them are listed and which one is on screen.
  const listed = page === "terminals" ? sessions : current && sandboxTab === "terminal" ? sessions.filter((s) => s.sandbox === current.name) : [];
  const onScreen = page === "terminals" ? (listed.find((s) => s.id === focused) ?? listed.at(-1)) : current ? listed.find((s) => s.id === shown(sessions, activeSession, current.name)) : undefined;
  const pick = (session: Session) => {
    setFocused(session.id);
    setActiveSession((active) => ({ ...active, [session.sandbox]: session.id }));
  };

  return (
    <div className={collapsed ? "layout collapsed" : "layout"}>
      <aside>
        <h1 className="brand">
          <img src="/icon.svg" alt="" width="28" height="28" />
          <span className="grow label">Boxquell</span>
          <button className="fold" title={collapsed ? "Show the sidebar" : "Fold the sidebar"} aria-label={collapsed ? "Show the sidebar" : "Fold the sidebar"} onClick={toggleSidebar}>
            {collapsed ? "›" : "‹"}
          </button>
        </h1>
        <nav>
          <NavItem icon={<LayoutDashboard size={17} />} active={page === "overview"} title="Overview" note="load and network" onClick={() => go("overview")} />
          <NavItem
            icon={<SquareTerminal size={17} />}
            active={page === "terminals"}
            title="Terminals"
            note={sessions.length === 0 ? "none open" : `${sessions.length} open`}
            badge={sessions.length || undefined}
            onClick={() => go("terminals")}
          />
          <NavItem
            icon={<Bell size={17} />}
            active={page === "notifications"}
            title="Notifications"
            note={blocked.length === 0 ? "nothing blocked" : `${blocked.length} blocked`}
            badge={blocked.length || undefined}
            onClick={() => go("notifications")}
          />
          <NavItem icon={<ChartColumn size={17} />} active={page === "usage"} title="Usage" note="tokens and costs" onClick={() => go("usage")} />
          <NavItem icon={<SettingsIcon size={17} />} active={page === "settings"} title="Settings" note={release?.newer ? `version ${(release.latest ?? "").replace(/^v/, "")} is available` : "plugins, secrets, network"}
            badge={release?.newer ? 1 : undefined}
            onClick={() => {
              if (release?.newer) setSettingsTab("about");
              go("settings");
            }}
          />
          <span className="nav-label">
            <span className="label">Sandboxes</span>
            <button className="add" title="New sandbox" aria-label="New sandbox" onClick={() => go("new")}>
              <Plus size={14} />
            </button>
          </span>
          {sandboxes.map((s) => (
            <Fragment key={s.name}>
            <div className={s.name === current?.name ? "item active" : "item"}>
              <button className="grow" onClick={() => go("sandbox", s.name)} title={s.name}>
                <span className={s.paused ? "dot paused" : s.running ? "dot on" : "dot"} />
                <span className="initials" aria-hidden="true">
                  {s.name.slice(0, 2)}
                </span>
                <span className="grow label">
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
                <Power size={13} />
              </button>
            </div>
            {/* Its open terminals, one line each. */}
            {sessions
              .filter((t) => t.sandbox === s.name)
              .map((t) => (
                <div key={t.id} className={onScreen?.id === t.id ? "subitem active" : "subitem"}>
                  <button
                    className="grow"
                    onClick={() => {
                      go("sandbox", s.name);
                      setTabs((all) => ({ ...all, [s.name]: "terminal" }));
                      pick(t);
                    }}
                  >
                    <span className="prompt" aria-hidden="true">
                      &gt;_
                    </span>
                    <span className="label">{t.title}</span>
                  </button>
                  <button className="power" title="Close this terminal" aria-label={`Close ${t.title} in ${s.name}`} onClick={() => closeTerminal(t.id)}>
                    ×
                  </button>
                </div>
              ))}
            </Fragment>
          ))}
          {sandboxes.length === 0 && <p className="muted pad">No sandboxes yet.</p>}
        </nav>
        <button
          className={lockdown ? "lockdown active" : "lockdown"}
          title="Blocks every connection of every sandbox at once and stops open ones"
          onClick={() => act(() => api.setLockdown(!lockdown))}
        >
          <ShieldAlert size={16} aria-hidden="true" />
          <span className="label">{lockdown ? "Lift network lockdown" : "Emergency stop"}</span>
        </button>
      </aside>
      <main>
        {outdated && (
          <div className="warning" role="alert">
            <strong>The running daemon is older than this page.</strong>
            <span>
              Some buttons will not work until it is restarted: run <code>boxquell daemon --stop</code>, then open the page again with <code>boxquell ui</code>.
              Stopping the daemon also stops running sandboxes.
            </span>
          </div>
        )}
        {lockdown && (
          <div className="error lockdown-banner" role="alert">
            <span>
              <strong>Network lockdown is on.</strong> No sandbox can reach the network, whatever its rules say.
            </span>
            <button onClick={() => act(() => api.setLockdown(false))}>Lift lockdown</button>
          </div>
        )}
        {error && (
          <div className="error" role="alert">
            {error}
            <button className="link" onClick={() => setError(null)}>
              dismiss
            </button>
          </div>
        )}

        {page === "overview" && <Overview sandboxes={sandboxes} open={(name) => go("sandbox", name)} act={act} />}

        {page === "sandbox" &&
          (current ? (
            <SandboxView
              key={current.name}
              sandbox={current}
              recipes={recipes}
              secrets={secrets}
              act={act}
              openTerminal={openTerminal}
              tab={tabs[current.name] ?? "overview"}
              setTab={(tab) => setTabs((all) => ({ ...all, [current.name]: tab }))}
            />
          ) : (
            <div className="empty">
              <p>There is no sandbox named {selected}.</p>
            </div>
          ))}

        {page === "usage" && <Usage act={act} secrets={secrets.map((secret) => secret.name)} />}
        {page === "notifications" && (
          <>
            <header className="hero">
              <div className="grow">
                <h2>Notifications</h2>
                <p className="muted">What your sandboxes tried to reach and were refused.</p>
              </div>
            </header>
            <Notifications
              blocked={blocked}
              dismiss={dismiss}
              settings={settings}
              saveSettings={saveSettings}
              act={act}
              open={(sandbox) => {
                go("sandbox", sandbox);
                setTabs((all) => ({ ...all, [sandbox]: "network" }));
              }}
            />
          </>
        )}

        {page === "new" && (
          <>
            <header className="hero">
              <div className="grow">
                <h2>New sandbox</h2>
                <p className="muted">A microVM for one project. Software can be added later as well.</p>
              </div>
            </header>
            <NewSandbox
              recipes={recipes}
              onClose={() => go("overview")}
              onCreated={(name) => {
                go("sandbox", name);
                refresh();
              }}
            />
          </>
        )}

        {page === "settings" && (
          <>
            <header className="hero">
              <div className="grow">
                <h2>Settings</h2>
                <p className="muted">For all sandboxes on this computer.</p>
              </div>
            </header>
            <div className="tabbar" role="tablist">
              {SETTINGS_TABS.map(([id, label]) => (
                <button key={id} role="tab" aria-selected={settingsTab === id} className={settingsTab === id ? "active" : ""} onClick={() => setSettingsTab(id)}>
                  {label}
                </button>
              ))}
            </div>
            {settingsTab === "general" && (
              <Preferences
                settings={settings}
                onChange={(next) => {
                  setSettings(next);
                  api.setSettings(next).then(setSettings, () => {});
                }}
              />
            )}
            {settingsTab === "agents" && <AgentSettings sandbox={null} act={act} />}
            {settingsTab === "mcp" && <Mcp settings={settings} onChange={saveSettings} act={act} />}
            {settingsTab === "plugins" && <Plugins onChange={() => api.recipes().then(setRecipes, () => {})} />}
            {settingsTab === "secrets" && (
              <>
                <Secrets secrets={secrets} act={act} />
                <SshKeys sandbox={null} act={act} />
              </>
            )}
            {settingsTab === "network" && <GlobalRules act={act} />}
            {settingsTab === "logs" && (
              <LogViewer title="Service log" about="What Boxquell's background service wrote: start-up, problems, and processes it had to end." load={api.daemonLog} />
            )}
            {settingsTab === "about" && <About />}
          </>
        )}

        {page === "terminals" && (
          <header className="hero">
            <div className="grow">
              <h2>Terminals</h2>
              <p className="muted">
                {sessions.length === 0 ? "No terminal is open. Open one from a sandbox, or start here:" : "Every open terminal, across all sandboxes. They stay connected while you look elsewhere."}
              </p>
            </div>
          </header>
        )}
        {page === "terminals" && (
          <div className="launch">
            {sandboxes.map((s) => (
              <button key={s.name} onClick={() => openTerminal(s.name, "Shell", undefined, true)}>
                <span className={s.paused ? "dot paused" : s.running ? "dot on" : "dot"} /> Shell in {s.name}
              </button>
            ))}
          </div>
        )}

        {/* All terminals live here, on every page, so none loses its connection. */}
        <section className={listed.length === 0 ? "hidden" : fullscreen ? "terminals full" : "terminals"}>
          <div className="tabs">
            {listed.map((s) => (
              <span key={s.id} className={onScreen?.id === s.id ? "tab active" : "tab"}>
                <button className="link" onClick={() => pick(s)}>
                  {page === "terminals" && <span className="muted">{s.sandbox} · </span>}
                  {s.title}
                </button>
                <button className="link" title="Close this terminal" aria-label={`Close ${s.title} in ${s.sandbox}`} onClick={() => closeTerminal(s.id)}>
                  ×
                </button>
              </span>
            ))}
            <span className="grow" />
            <button onClick={() => setFullscreen((now) => !now)}>{fullscreen ? "Exit full screen" : "Full screen"}</button>
          </div>
          {sessions.map((s) => (
            <Terminal key={s.id} session={s} visible={onScreen?.id === s.id} onExit={() => {}} />
          ))}
        </section>
      </main>
      <Toasts
        enabled={settings.toast_blocked}
        ignored={settings.ignore_blocked}
        onChange={refresh}
        onHandled={dismiss}
        onOpen={(sandbox) => {
          go("sandbox", sandbox);
          setTabs((all) => ({ ...all, [sandbox]: "network" }));
        }}
      />
    </div>
  );
}

type Page = "overview" | "terminals" | "notifications" | "usage" | "settings" | "new" | "sandbox";
type SettingsTab = "general" | "agents" | "mcp" | "plugins" | "secrets" | "network" | "logs" | "about";
const SETTINGS_TABS: [SettingsTab, string][] = [
  ["general", "General"],
  ["agents", "Agents"],
  ["mcp", "MCP"],
  ["plugins", "Plugins"],
  ["secrets", "Secrets"],
  ["network", "Network"],
  ["logs", "Logs"],
  ["about", "About"],
];

function NavItem(props: { icon: React.ReactNode; active: boolean; title: string; note: string; badge?: number; onClick: () => void }) {
  return (
    <div className={props.active ? "item active" : "item"}>
      <button className="grow" onClick={props.onClick} title={props.title}>
        <span className="glyph" aria-hidden="true">
          {props.icon}
        </span>
        <span className="grow label">
          <strong>{props.title}</strong>
          <small>{props.note}</small>
        </span>
        {props.badge !== undefined && <span className="badge">{props.badge}</span>}
      </button>
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
