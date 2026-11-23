# Changelog

All notable changes to Boxquell. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- Ready-made system images: a sandbox downloads its system on first use, checked against checksums, so Docker is no longer needed to get started.
- Building a system image works with Docker alone, on Windows and macOS too.
- Installers for Linux, macOS and Windows, a Homebrew formula, and a software bill of materials with every release.
- A tray icon for the desktop app.
- SSH keys for git: sandboxes can sign with keys of this computer, chosen for all sandboxes or for one, without the keys entering a sandbox (`boxquell ssh-keys`, and under Secrets in the UI).
- The overview of a sandbox names the system it runs.
- Icons for software and systems, taken from their repositories, and a search field in the software catalogue.
- Choosing the operating system of a new sandbox from cards instead of a list.

## [0.0.1] - 2026-11-23

First public version, for testing. Built and tested on Linux; the macOS and Windows binaries are untested and not signed yet.

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
- A tray icon for the desktop app: open, emergency stop, quit.
- Desktop app and web interface: overview with load and network monitoring, terminals, software catalogue, settings.
- A notifications page for blocked connections: allow, ignore or dismiss them.
- Logs, version, update check, changelog and third-party notices inside the interface.
- Shell completion for bash, zsh, fish and PowerShell.

[0.0.1]: https://github.com/Boxquell/boxquell/releases/tag/v0.0.1
