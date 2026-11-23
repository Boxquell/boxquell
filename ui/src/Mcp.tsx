import { Check, Copy, ExternalLink, Plug } from "lucide-react";
import { useEffect, useState } from "react";
import { api, type Settings } from "./api";

type Act = (action: () => Promise<unknown>) => Promise<void>;

const TOOLS: [string, string, string][] = [
  ["list_sandboxes, sandbox_info, list_software", "Look at sandboxes and at what can be installed", "always"],
  ["network_log", "See what a sandbox reached and what was blocked", "always"],
  ["list_tasks, task_log, review_changes, review_diff", "Follow tasks, and see what a sandbox in review mode changed", "always"],
  ["create_sandbox, start_sandbox, stop_sandbox", "Create, start and stop sandboxes", "unless read only"],
  ["run_command, install_software", "Run commands and install software in a sandbox", "unless read only"],
  ["give_task, stop_task, take_snapshot", "Give an agent in a sandbox a task; save a sandbox's state", "unless read only"],
  ["allow_destination, deny_destination", "Change a sandbox's network rules", "when allowed below"],
  ["review_apply", "Take a sandbox's changes in review mode into your project folder", "when allowed below"],
  ["delete_sandbox", "Delete a sandbox with its disk", "when allowed below"],
];

/** A piece of text to copy into a terminal or a configuration file. */
function Snippet(props: { title: string; where?: string; text: string }) {
  const [copied, setCopied] = useState(false);
  return (
    <div className="snippet">
      <div className="snippet-head">
        <span className="grow">
          <strong>{props.title}</strong>
          {props.where && <small className="muted"> {props.where}</small>}
        </span>
        <button
          className="link"
          onClick={() =>
            navigator.clipboard.writeText(props.text).then(() => {
              setCopied(true);
              setTimeout(() => setCopied(false), 1500);
            })
          }
        >
          {copied ? <Check size={14} aria-hidden /> : <Copy size={14} aria-hidden />} {copied ? "Copied" : "Copy"}
        </button>
      </div>
      <pre>{props.text}</pre>
    </div>
  );
}

/** Connecting an assistant on this computer to Boxquell through the Model Context Protocol, and what it may do. */
export function Mcp(props: { settings: Settings; onChange: (settings: Settings) => void; act: Act }) {
  const { settings, act } = props;
  const [info, setInfo] = useState<{ exe: string | null; claude: boolean; claude_connected: boolean } | null>(null);
  useEffect(() => {
    api.mcp().then(setInfo, () => {});
  }, []);
  const exe = info?.exe ?? "boxquell";
  const toggle = (key: "mcp_read_only" | "mcp_network_rules" | "mcp_delete" | "mcp_review_apply") => props.onChange({ ...settings, [key]: !settings[key] });
  const quoted = exe.includes(" ") ? `"${exe}"` : exe;
  const json = JSON.stringify({ mcpServers: { boxquell: { command: exe, args: ["mcp"] } } }, null, 2);
  const row = (key: "mcp_read_only" | "mcp_network_rules" | "mcp_delete" | "mcp_review_apply", title: string, help: string, off?: boolean) => (
    <label className="check setting">
      <input type="checkbox" className="switch" checked={settings[key]} disabled={off} onChange={() => toggle(key)} />
      <span>
        {title}
        <small className="muted block">{help}</small>
      </span>
    </label>
  );

  return (
    <div className="pane">
      <h3>
        <Plug size={16} aria-hidden /> Assistants on this computer
      </h3>
      <p className="muted">
        Boxquell speaks the Model Context Protocol (MCP). An assistant that runs on this computer, such as Claude Code, Codex or an editor, can then work with
        your sandboxes: create one for a project, run commands in it, give an agent in it a task, and read the network log when something is blocked. The
        assistant runs here; the work happens in the sandbox.
      </p>

      <h3>Connect</h3>
      {info?.claude && (
        <div className="notice">
          <Plug size={18} aria-hidden />
          <span className="grow">
            <strong>Claude Code {info.claude_connected ? "is connected to Boxquell" : "is on this computer"}</strong>
            <small className="muted block">
              {info.claude_connected
                ? "New Claude Code sessions have the Boxquell tools. Ask it, for example, to create a sandbox for the project and run the tests there."
                : "One click adds Boxquell to it for your user. Sessions you start afterwards have the tools."}
            </small>
          </span>
          <button className={info.claude_connected ? "" : "primary"} onClick={() => act(() => api.connectMcp(!info.claude_connected).then(setInfo))}>
            {info.claude_connected ? "Disconnect" : "Add to Claude Code"}
          </button>
        </div>
      )}
      <p className="muted">By hand, in the assistant's own configuration. The server is the Boxquell program itself with the word <code>mcp</code>:</p>
      <Snippet title="Claude Code" where="in a terminal" text={`claude mcp add --scope user boxquell -- ${quoted} mcp`} />
      <Snippet title="Codex" where="in ~/.codex/config.toml" text={`[mcp_servers.boxquell]\ncommand = ${JSON.stringify(exe)}\nargs = ["mcp"]`} />
      <Snippet title="Cursor, Claude Desktop and others" where="in their MCP configuration file (JSON)" text={json} />

      <h3>What assistants may do</h3>
      <p className="muted">
        These switches apply to every connected assistant at once, from its next request on. The assistant's own program decides whether it asks you before
        each step; Boxquell does not ask again. Secrets have no tool: an assistant cannot read, set or attach keys.
      </p>
      {row("mcp_read_only", "Look only", "No creating, starting, running, installing or tasks. This wins over the switches below.")}
      {row("mcp_network_rules", "Change network rules", "Allow and deny destinations for a sandbox. Off, an assistant can tell you what was blocked and you decide.", settings.mcp_read_only)}
      {row(
        "mcp_review_apply",
        "Take over changes in review mode",
        "Copy what a sandbox changed into your project folder. Off, taking changes over stays your step on the Review tab.",
        settings.mcp_read_only,
      )}
      {row("mcp_delete", "Delete sandboxes", "Remove a sandbox with its disk. The project folder is never touched.", settings.mcp_read_only)}

      <h3>Tools</h3>
      <table className="sums">
        <thead>
          <tr>
            <th>Tool</th>
            <th>What it does</th>
            <th>Offered</th>
          </tr>
        </thead>
        <tbody>
          {TOOLS.map(([names, what, when]) => (
            <tr key={names}>
              <td>
                <code>{names}</code>
              </td>
              <td>{what}</td>
              <td className="muted">{when}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="muted">
        <a href="https://github.com/Boxquell/boxquell/wiki/MCP-Server" target="_blank" rel="noreferrer">
          More in the wiki <ExternalLink size={13} aria-hidden />
        </a>
      </p>
    </div>
  );
}
