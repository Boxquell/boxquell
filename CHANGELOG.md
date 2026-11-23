# Changelog

All notable changes to Boxquell. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## Unreleased

First public version.

### Sandboxes
- MicroVM sandboxes on libkrun and QEMU: create, start, stop, pause, resize, delete.
- A choice of operating system (Debian, Ubuntu, Alpine), built from definitions in a repository.
- Shared folders, a project root, environment variables, and agents or commands that start with the sandbox.
- Terminal access from the command line, the interface and over ssh (`ssh boxquell-<name>`), plus port forwards.

### Network
- Closed by default: rules for names, wildcards, addresses and networks; whitelist and blacklist mode.
- Rules for all sandboxes, and an emergency stop that cuts every sandbox off at once.
- A log of every connection attempt, with notifications when something is blocked.

### Secrets and agents
- API keys stay on the host and are put into requests on their way out; shared or per sandbox.
- Claude Code settings for LLM gateways such as LiteLLM, with the token kept on the host.
- Instructions, skills and MCP servers for the agents, for all sandboxes or one, with import from this computer.

### Software
- Recipes for agents, languages, tools and IDEs in their own repositories, with selectable versions.
- Add your own repositories next to the official ones.

### Interface
- Desktop app and web interface: overview with load and network monitoring, terminals, software catalogue, settings.
- A notifications page for blocked connections: allow, ignore or dismiss them.
- Logs, version, update check, changelog and third-party notices inside the interface.
- Shell completion for bash, zsh, fish and PowerShell.
