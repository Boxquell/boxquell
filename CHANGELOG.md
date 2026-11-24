# Changelog

All notable changes to Boxquell. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Fixed
- Failed SSH-agent forwarding now reports `ssh` errors in the daemon log instead of hiding them.
- DNS filtering logs private and local IP addresses returned for an allowed hostname, and returns `NXDOMAIN` when all addresses are blocked.
- Linux desktop builds cache Ubuntu packages on `main`, retry slow apt downloads, and switch mirrors after repeated failures.

### Changed
- The network log no longer offers a host-name allow action for a blocked private IP; Claude Code settings show a clear restart notice while the sandbox is running.

## [0.1.1] - 2026-11-24

### Added
- The start page shows this computer's swap: as a second line in the memory trend and as a bar of its own.
- `boxquell doctor` says when libkrun is there but libkrunfw is not.
- The QEMU backend gives a sandbox its project folder and shared folders, through QEMU's own file server (9p). It needs the current Debian image, whose kernel can mount them: `boxquell os install debian`, then a new sandbox. Not on Windows, where QEMU has no file server; use Boxquell in WSL 2 there.
- `boxquell daemon --restart` starts the daemon anew, and the install script does so after an update, so the new version is the one that runs. A daemon with running sandboxes is left alone and named; `--force` restarts it anyway, `install.sh --no-restart` never does.

### Fixed
- macOS: a sandbox did not start with "Couldn't find or load libkrunfw". The signed program could not see the folder libkrun and its kernel are installed in; it now may.

### Changed
- An install script gets a temporary folder on the sandbox's disk that is removed afterwards. Downloads no longer stay in `/tmp`, which is memory in a sandbox.

## [0.1.0] - 2026-11-23

### Changed
- A new interface. A sandbox's page has five areas (Overview, Work, Software, Network, Settings) with their parts below, instead of eleven tabs; its head shows the system, the state and the facts, with a Terminal button always at hand. The start page shows the sandboxes as cards with their state and quick actions, and greets a first start with the three steps to a working sandbox. The sidebar is one line per entry, with Settings and the emergency stop at the bottom. Colours, type and controls follow one system, in light and dark.
- Speed and data limits are set with sliders.

### Added
- Find anything with Ctrl+K (⌘K on a Mac): jump to a sandbox, an area or a settings page, or start, stop and open a terminal.
- Actions: a recipe can offer commands of its software as buttons on the Config tab, with fields for the values they need, for example the Jira CLI's first setup (`[[action]]`).
- MCP: a Settings › MCP page that connects Claude Code with one button and has the lines to copy for other assistants, switches for what assistants may do (also `boxquell config mcp-…`), and new tools for tasks, snapshots and review mode.
- Keys in HTTP Basic sign-in (user name plus token, as Jira and Bitbucket use it) are put in on the way out like keys in other headers.
- Speed limits per sandbox for sending and receiving (`boxquell set box --download-speed 2000`, in kbit per second, and under Network); they apply at once, also to open connections.
- Traffic statistics per sandbox under Network: what it sent and received per hour or day, and the destinations that carried most of it.
- Review mode: the sandbox works on a copy of the project folder, and a Review tab shows what it changed, file by file and line by line. Changes are taken into the project or discarded, all at once or per file; files that changed in the project meanwhile are told apart (`boxquell review`, `boxquell new --review`).
- Tasks: give an installed agent a task to work through in the background, without a terminal (`boxquell task box "…"`, and a Tasks tab per sandbox with the log of each task). A snapshot of the sandbox can be taken first, and a notification says when the task is over. Agent recipes name the command for it with `task`.
- Usage: Boxquell reads the tokens a request to a model API used from its answer as it passes (Anthropic and OpenAI formats, also streams) and shows them on a Usage page: a chart per hour or day, sums per key, sandbox and model, and an estimated cost from an editable price list.
- Limits per key and day, in dollars or tokens: a key that reaches its limit is refused until the next day, with an answer that says so.
- Project file: a `boxquell.toml` in a project folder describes its sandbox (system, software, rules, ports, variables). `boxquell up` in the folder creates and starts it, `boxquell init` writes the file from a sandbox, and the form for a new sandbox offers to use the file of the chosen folder.
- Idle stop: a sandbox with no terminal open, hardly any CPU use and no data to or from outside is stopped after a time you set, for all sandboxes (`boxquell config idle-stop-minutes 30`, Settings › General) or for one (`boxquell set box --idle-stop 60`, its Settings).
- Rules that end by themselves: `boxquell allow box example.com --for 2h`, a choice of duration in the rule form, and "for 1 hour" next to Allow in the notifications.
- Read-only rules: `boxquell read box example.com` lets web requests that fetch through and refuses those that send or change something.
- Data limits per sandbox (`boxquell set box --upload-limit 200 --download-limit 2000`, and under Network): beyond a limit the sandbox's network is cut off until you let it continue.
- Snapshots: save the state of a sandbox's disk and go back to it (`boxquell snapshot take|ls|restore|rm`, and under a sandbox's Settings).
- Templates: save a sandbox as a starting point and create new ones from it, with its system and software already there (`boxquell template save|ls|rm`, `boxquell new --template`, and "Start from" when creating a sandbox).
- Keys for installed software are entered on the Config tab, next to its settings; recipes describe them with `[[credential]]`.
- Software can be removed again (`boxquell remove`, or Remove on its card) and updated when its recipe offers a newer version or has changed (`boxquell add --again`, or Update on its card).
- A recipe can require a version of another: `requires = ["node>=22"]`.
- A Config tab per sandbox: installed software shows its settings as fields (recipes describe them with `[[config]]`), next to the variables set by hand. Changes reach a running sandbox at once.
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
- Installing software no longer removes a rule of yours for a destination the install opens for a moment (for example `read github.com`): the rule gives way while the script runs and is put back afterwards.
- The Config tab showed the names of secrets instead of the labels, help texts and links their recipes give.
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
[0.1.0]: https://github.com/Boxquell/boxquell/releases/tag/v0.1.0
[0.1.1]: https://github.com/Boxquell/boxquell/releases/tag/v0.1.1
