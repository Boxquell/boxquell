# Boxquell as an MCP server

`boxquell mcp` speaks the [Model Context Protocol](https://modelcontextprotocol.io) on standard input and output. An assistant that runs on your computer, such as Claude Code, Codex or an editor, can then work with your sandboxes: create one for a project, run commands in it, and read the network log when something is blocked.

The assistant runs on the host; the work happens in the sandbox. That is the point: commands the assistant runs through Boxquell see only the sandbox, its `/workspace` and the network destinations you allowed.

## Set it up

The server is the `boxquell` program itself, so there is nothing else to install.

**Claude Code**

```sh
claude mcp add boxquell -- boxquell mcp
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

| Tool | What it does |
|---|---|
| `list_sandboxes` | The sandboxes with their state |
| `sandbox_info` | One sandbox in full: state, system, resources, software, network mode and rules, ports, shared folders, warnings |
| `list_software` | Agents, languages, tools and IDE servers that can be installed |
| `network_log` | Recent connection attempts of a sandbox: destination, allowed or blocked, and the rule that decided |
| `create_sandbox` | A new sandbox, optionally with a project folder, a system, resources and software |
| `start_sandbox`, `stop_sandbox` | Start and stop |
| `run_command` | Run a shell command in a sandbox and return its output and exit status |
| `install_software` | Install software from the catalogue |
| `allow_destination`, `deny_destination` | Change network rules. Only with `--allow-network-rules` |
| `delete_sandbox` | Delete a sandbox with its disk. Only with `--allow-delete` |

A command that fails comes back as an error with its output, so the assistant can read what went wrong. Output beyond 60 000 bytes is cut at the front. `run_command` gives up after 120 seconds unless `timeout_seconds` says otherwise (at most 1800); what it started in the sandbox may keep running there.

## What the assistant may do

By default the assistant can look, create, start, stop, run and install. Two things are off until you switch them on, because they are decisions about your computer rather than about the work:

| Start the server with | To also offer |
|---|---|
| `boxquell mcp --allow-network-rules` | `allow_destination` and `deny_destination` |
| `boxquell mcp --allow-delete` | `delete_sandbox` |
| `boxquell mcp --read-only` | Nothing but the tools that look; this wins over the other two |

Without `--allow-network-rules` an assistant that hits a blocked destination can tell you which one it was (`network_log`), and you allow it in the app or with `boxquell allow`.

Things to know:

- **Secrets have no tool.** The assistant cannot read, set or attach API keys. `sandbox_info` lists the names of attached secrets, never their values.
- **A project folder is shared read-write.** `create_sandbox` with a `workspace` gives the sandbox that folder, as `boxquell new --workspace` does. Commands in the sandbox can change files in it.
- **With `--allow-network-rules` the assistant can open the network for a sandbox.** Text the assistant reads, for example in a web page or a repository, could talk it into allowing a destination. Leave the flag off unless you want that.
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
