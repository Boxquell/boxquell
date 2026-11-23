# Changelog

All notable changes to Boxquell. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- The overview shows the load of this computer itself: CPU, memory and the disk that holds the sandboxes.
- What installed software and the system's package manager need is open by itself, shown under Network and to be switched off per destination; before an install, a sheet lists what comes along and which destinations it opens.
- A Block button for allowed connections in the network log.
- The software catalogue in the interface follows changes to the repositories without a reload.
- Claude Code settings with a key helper (`apiKeyHelper`), as used for sign-in through a company account: the helper keeps running on this computer and its key is put into the sandbox's requests, refreshed before it runs out. This computer's managed settings are taken over with their drop-in files (`managed-settings.d`), in the UI with one button.
- A model field per agent in a sandbox's Agent tab, for agents whose recipe names the variable (`model_env`).
- `boxquell mcp`: a Model Context Protocol server, so an assistant on this computer can create sandboxes, run commands in them and read the network log ([docs/mcp.md](docs/mcp.md)).
- The desktop app is part of releases, and the installers set it up with an entry in the application menu (Linux), Applications (macOS) or the Start menu (Windows).
- The installers offer to install what is missing (libkrun or QEMU, an SSH client, access to KVM) with the system's package manager.
- The desktop app finds the `boxquell` command where the installers and Homebrew put it, also when started from the menu.
- SSH keys for git: sandboxes can sign with keys of this computer, chosen for all sandboxes or for one, without the keys entering a sandbox (`boxquell ssh-keys`, and under Secrets in the UI).
- The overview of a sandbox names the system it runs.
- Icons for software and systems, taken from their repositories, and a search field in the software catalogue.
- Choosing the operating system of a new sandbox from tiles, with a version chooser where a system comes in several versions.
- Icons carry their brand colour (`icon_color`).
- Repositories are fetched once a day, and Boxquell looks for a newer release once a day and says so with a notification and in the sidebar. Both can be switched off under Settings › General.
- Software that is installed already is greyed out in the catalogue.

### Fixed
- Notifications that were dismissed or allowed no longer come back when the interface is opened again.
- Environment variables of a recipe (`[env]`) were not set in login shells on Debian.

## [0.0.2] - 2026-11-23

### Added
- Ready-made system images: a sandbox downloads its system on first use, checked against checksums, so Docker is no longer needed to get started.
- Building a system image works with Docker alone, on Windows and macOS too.
- Installers for Linux, macOS and Windows, a Homebrew formula, and a software bill of materials with every release.
- A tray icon for the desktop app.

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
