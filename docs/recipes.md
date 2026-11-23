# Recipes and recipe repositories

Agents, languages and tools are installed into a sandbox by **recipes**. Boxquell ships none itself: on first use it adds three official repositories, [agents](https://github.com/Boxquell/boxquell-agents), [languages](https://github.com/Boxquell/boxquell-languages) and [tools](https://github.com/Boxquell/boxquell-tools), and you can add any number of others.

```sh
boxquell repo ls
boxquell repo add https://example.com/someone/boxquell-extras
boxquell repo update
boxquell repo rm extras
```

A recipe is named `<repository>:<id>`, for example `languages:node`. The short form `node` works when it is unambiguous; the official repositories win over others, so a third-party repository cannot take over a well-known name.

## Make your own repository

A recipe repository is any git repository with `*.toml` files in a `recipes` directory (or at its top level). The file name is the recipe's id. Add it with `boxquell repo add <git URL or local path>`; its recipes are then named `<repository>:<id>`.

```toml
name = "Node.js"                       # shown in lists
category = "language"                  # agent, language, tool or ide
description = "Node.js with npm"
icon = "nodedotjs.svg"                 # optional; a file in the repository's icons/ directory
icon_color = "#5FA04E"                 # optional; the brand's colour behind the icon

versions = ["24", "22"]                # optional; first is the default, chosen as node@22
os = ["debian", "ubuntu"]             # optional; systems the script works on, debian when left out
requires = ["build-essential"]         # optional; installed first (same repository first, then the others)
install_allow = ["nodejs.org", "@packages"]   # open only while the install script runs; @packages = the system's package hosts
allow = ["registry.npmjs.org"]         # destinations the software needs afterwards
secrets = ["anthropic"]                # optional; host secrets this software can use
command = "node"                       # agents only: the command that starts it
ports = [3000]                         # optional; forwarded from the same port on 127.0.0.1 of the host

install = '''
# Runs in the sandbox as user `dev` with passwordless sudo, under `set -euo pipefail`.
# $BOXQUELL_VERSION holds the chosen version when the recipe has versions.
curl -fsSL "https://nodejs.org/dist/latest-v$BOXQUELL_VERSION.x/..." | ...
'''

[env]                                  # optional; variables for sessions in the sandbox
EXAMPLE = "1"
```

Rules of thumb:

- List every host the install script contacts in `install_allow`. Anything else is blocked, and `boxquell logs <sandbox> --blocked` shows what was refused.
- Keep `allow` as small as possible: these destinations are open for a sandbox for as long as the recipe is installed. Users see them before the install and can switch each one off under Network.
- The guest is Debian. Install to system paths (`/usr/local`) so every session finds the software.
- An icon is a small single-colour SVG (under 64 KB, no scripts or links); the UI paints it in the text colour. System definitions in `os/` take the same `icon` line.
- An agent recipe can name the variable that selects its model, `model_env = "ANTHROPIC_MODEL"`; the UI then offers a model field for it.
- Never put credentials into a recipe. Name a secret instead; its value stays on the host.

Recipes run their own scripts inside the sandbox and open the destinations they list. Users are told so when they add a repository, and should add only repositories they trust.
