# Boxquell as an MCP server

`boxquell mcp` speaks the [Model Context Protocol](https://modelcontextprotocol.io) on standard input and output. An assistant that runs on your computer, such as Claude Code, Codex or an editor, can then work with your sandboxes: create one for a project, run commands in it, and read the network log when something is blocked.

The assistant runs on the host; the work happens in the sandbox. That is the point: commands the assistant runs through Boxquell see only the sandbox, its `/workspace` and the network destinations you allowed.

## Set it up

The server is the `boxquell` program itself, so there is nothing else to install.

**In the app:** Settings › MCP. When Claude Code is on your computer, one button adds Boxquell to it; for other assistants the page has the lines to copy, with the full path of the program filled in. The same page has the switches for what assistants may do.

**Claude Code**

```sh
claude mcp add --scope user boxquell -- boxquell mcp
```

**Codex** (`~/.codex/config.toml`)

```toml
[mcp_servers.boxquell]
command = "boxquell"
args = ["mcp"]
```

**Clients with a JSON configuration** (Claude Desktop, Cursor and others)

```json
{
  "mcpServers": {
    "boxquell": {
      "command": "boxquell",
      "args": ["mcp"]
    }
  }
}
```

If the client does not find `boxquell`, give the full path, e.g. `~/.local/bin/boxquell`. The server uses the same sandboxes and the same background service as the command line and the app, and starts the service when it is not running.

## Tools

| Tool | What it does | Offered |
|---|---|---|
| `list_sandboxes` | The sandboxes with their state | always |
| `sandbox_info` | One sandbox in full: state, system, resources, software, network mode and rules, ports, shared folders, warnings | always |
| `list_software` | Agents, languages, tools and IDE servers that can be installed | always |
| `network_log` | Recent connection attempts of a sandbox: destination, allowed or blocked, and the rule that decided | always |
| `list_tasks`, `task_log` | The tasks given to a sandbox's agents, and what one printed | always |
| `review_changes`, `review_diff` | For a sandbox in review mode: the files it changed, and one file line by line | always |
| `create_sandbox` | A new sandbox, optionally with a project folder, a system, resources and software | unless look only |
| `start_sandbox`, `stop_sandbox` | Start and stop | unless look only |
| `run_command` | Run a shell command in a sandbox and return its output and exit status | unless look only |
| `install_software` | Install software from the catalogue | unless look only |
| `give_task`, `stop_task` | Give an agent in the sandbox a task to work through in the background; stop one | unless look only |
| `take_snapshot` | Save the state of a sandbox's disk | unless look only |
| `allow_destination`, `deny_destination` | Change network rules | when switched on |
| `review_apply` | Take a sandbox's changes in review mode into the project folder | when switched on |
| `delete_sandbox` | Delete a sandbox with its disk | when switched on |

A command that fails comes back as an error with its output, so the assistant can read what went wrong. Output beyond 60 000 bytes is cut at the front. `run_command` gives up after 120 seconds unless `timeout_seconds` says otherwise (at most 1800); what it started in the sandbox may keep running there.

## What the assistant may do

By default the assistant can look, create, start, stop, run, install, give tasks and take snapshots. Four things are decisions about your computer rather than about the work, and have a switch each under Settings › MCP (`boxquell config` has the same names). A switch applies to every connected assistant from its next request on:

| Switch | `boxquell config …` | Starting the server with | Effect |
|---|---|---|---|
| Look only | `mcp-read-only on` | `--read-only` | Nothing but the tools that look; this wins over the others |
| Change network rules | `mcp-network-rules on` | `--allow-network-rules` | `allow_destination` and `deny_destination` |
| Take over changes in review mode | `mcp-review-apply on` | `--allow-review-apply` | `review_apply` |
| Delete sandboxes | `mcp-delete on` | `--allow-delete` | `delete_sandbox` |

Without "change network rules" an assistant that hits a blocked destination can tell you which one it was (`network_log`), and you allow it in the app or with `boxquell allow`. Without "take over changes" an assistant can show you what a sandbox in review mode changed, and taking it over stays your step.

Things to know:

- **Secrets have no tool.** The assistant cannot read, set or attach API keys. `sandbox_info` lists the names of attached secrets, never their values.
- **A project folder is shared read-write** unless the sandbox is in review mode. `create_sandbox` with a `workspace` gives the sandbox that folder, as `boxquell new --workspace` does. Commands in the sandbox can change files in it.
- **With "change network rules" the assistant can open the network for a sandbox.** Text the assistant reads, for example in a web page or a repository, could talk it into allowing a destination. Leave it off unless you want that.
- **Run the server on your computer, not inside a sandbox.** Inside, there is no Boxquell to talk to.
- The client decides whether it asks you before each tool call. Boxquell does not ask again.

## Try it by hand

The protocol is one JSON message per line:

```sh
printf '%s\n' \
  '{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"me","version":"1"}}}' \
  '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"list_sandboxes","arguments":{}}}' \
  | boxquell mcp
```
