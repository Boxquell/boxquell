import { useEffect, useState } from "react";
import { api, type AgentConfig, type Found, type Suggestions } from "./api";

type Act = (action: () => Promise<unknown>) => Promise<void>;

/**
 * Instructions, skills and MCP servers for the agents. `sandbox` null edits the
 * set for all sandboxes; a name edits what that sandbox gets on top of it.
 */
export function AgentSettings(props: { sandbox: string | null; act: Act }) {
  const { sandbox, act } = props;
  const [config, setConfig] = useState<AgentConfig | null>(null);
  const [instructions, setInstructions] = useState("");
  const [mcp, setMcp] = useState("");
  const [skillPath, setSkillPath] = useState("");

  const [found, setFound] = useState<Suggestions>({ instructions: [], skills: [], mcp: [] });
  const show = (next: AgentConfig) => {
    setConfig(next);
    setInstructions(next.instructions);
    setMcp(next.mcp);
    // What is on offer depends on what the set already has.
    api.agentSuggestions(sandbox).then(setFound, () => {});
  };
  useEffect(() => {
    api.agentConfig(sandbox).then(show, () => {});
  }, [sandbox]);
  const take = (items: { instructions?: string[]; skills?: string[]; mcp?: string[] }) => act(() => api.agentImport(sandbox, items).then(show));

  if (!config) return <section className="centered muted">Loading…</section>;
  const changed = instructions !== config.instructions || mcp !== config.mcp;
  const scope = sandbox ? `Only ${sandbox} gets this, on top of what is set for all sandboxes.` : "Every sandbox gets this. A sandbox can add its own on top.";

  return (
    <>
      <section>
        <h3>Instructions</h3>
        <p className="muted">
          {scope} Agents read it as their global instructions: CLAUDE.md for Claude Code, AGENTS.md for Codex, opencode and pi.
        </p>
        <Offers title="Found on this computer" items={found.instructions} label={(f) => f.origin} onTake={(f) => take({ instructions: [f.origin] })} />
        <textarea rows={8} spellCheck={false} placeholder={"# How to work here\n\n- Answer briefly.\n- Run the tests before you say something works."} value={instructions} onChange={(e) => setInstructions(e.target.value)} />
      </section>

      <section>
        <h3>MCP servers</h3>
        <p className="muted">
          JSON with one entry per server. They are registered with Claude Code when the sandbox starts, and written to ~/.config/boxquell/mcp.json for
          other agents. A server that talks to the network needs its hosts allowed. Tokens written here enter the sandbox as they are; use the
          placeholder of a secret instead.
        </p>
        <Offers
          title="Known to your agents on this computer"
          items={found.mcp}
          label={(f) => `${f.name}${f.detail ? ` · ${f.detail}` : ""}${f.has_secrets ? " · carries environment values or headers" : ""}`}
          onTake={(f) => take({ mcp: [f.name] })}
        />
        <textarea
          rows={8}
          spellCheck={false}
          placeholder={'{\n  "docs": { "command": "npx", "args": ["-y", "some-mcp-server"] },\n  "tracker": { "type": "http", "url": "https://mcp.example.com/mcp" }\n}'}
          value={mcp}
          onChange={(e) => setMcp(e.target.value)}
        />
        <div className="actions left">
          <button className="primary" disabled={!changed} onClick={() => act(() => api.setAgentConfig(sandbox, instructions, mcp).then(show))}>
            Save instructions and servers
          </button>
          {changed && <button onClick={() => show(config)}>Discard changes</button>}
        </div>
      </section>

      <section>
        <h3>Skills</h3>
        <p className="muted">Folders with a SKILL.md. They are copied into ~/.claude/skills and ~/.agents/skills of the sandbox.</p>
        <ul className="rules">
          {config.skills.map((skill) => (
            <li key={skill}>
              <code>{skill}</code>
              <button className="link" onClick={() => act(() => api.removeSkill(sandbox, skill).then(show))}>
                remove
              </button>
            </li>
          ))}
          {config.skills.length === 0 && <li className="muted">none</li>}
        </ul>
        <Offers title="Found on this computer" items={found.skills} label={(f) => f.name} onTake={(f) => take({ skills: [f.name] })} all={() => take({ skills: found.skills.map((f) => f.name) })} />
        <form
          className="row"
          onSubmit={(event) => {
            event.preventDefault();
            act(() => api.addSkill(sandbox, skillPath.trim()).then(show)).then(() => setSkillPath(""));
          }}
        >
          <label className="grow">
            Skill folder on this computer
            <input required placeholder="/home/me/.claude/skills/my-skill" value={skillPath} onChange={(e) => setSkillPath(e.target.value)} />
          </label>
          <button>Add skill</button>
        </form>
        <p className="muted">
          Stored in <code>{config.path}</code>. A sandbox picks changes up the next time it starts.
        </p>
      </section>
    </>
  );
}

/** Things that exist on this computer already, each with a button to take it over. */
function Offers(props: { title: string; items: Found[]; label: (item: Found) => string; onTake: (item: Found) => void; all?: () => void }) {
  if (props.items.length === 0) return null;
  return (
    <div className="offers">
      <span className="muted">
        {props.title}
        {props.all && props.items.length > 1 && (
          <button className="link" onClick={props.all}>
            add all {props.items.length}
          </button>
        )}
      </span>
      <ul className="rules">
        {props.items.map((item) => (
          <li key={`${item.origin}/${item.name}`} title={`${item.origin}\n${item.detail}`}>
            {props.label(item)}
            <button className="link" onClick={() => props.onTake(item)}>
              add
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}
