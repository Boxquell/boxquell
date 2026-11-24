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
ports = [3000]                         # optional fixed host-to-guest forwards
port_env = "APP_PORT"                  # optional managed service port; must match a config field with kind="port"

install = '''
# Runs in the sandbox as user `dev` with passwordless sudo, under `set -euo pipefail`.
# $BOXQUELL_VERSION holds the chosen version when the recipe has versions.
curl -fsSL "https://nodejs.org/dist/latest-v$BOXQUELL_VERSION.x/..." | ...
'''

[env]                                  # optional; variables for sessions in the sandbox
EXAMPLE = "1"
```

### Requirements, removal and updates

- `requires = ["node>=22"]` asks for that version or a newer one, `requires = ["node@22"]` for exactly that one, `requires = ["node"]` for any. The install picks a version of the required recipe that fits, or stops and says why.
- `uninstall = '''…'''` is a script that takes the software out again (`boxquell remove <sandbox> <recipe>`, or Remove on its card). Without one, removing a recipe only takes it off the list. Software that something else installed requires cannot be removed first.
- When a recipe offers a newer version than the one installed, or the recipe file changed since the install, its card offers Update (`boxquell add <sandbox> <recipe> --again`).

### Credentials

A recipe names the secrets it can use with `secrets = ["gitlab"]`. Describe each one, and the Config tab offers a field for its value:

```toml
[[credential]]
name = "gitlab"
label = "GitLab access token"
help = "A personal access token with the api scope."
url = "https://gitlab.com/-/user_settings/personal_access_tokens"
hosts = ["gitlab.com"]                 # where the real value may be sent
env = ["GITLAB_TOKEN"]                 # variables that hold the placeholder in the sandbox
```

The names `anthropic`, `claude-oauth`, `openai`, `openrouter` and `github` are built in; for them `hosts` and `env` can be left out. A credential works where the software sends the variable's value in a request header to those hosts.

### Actions

A command of the installed software can be offered as a button on the Config tab, for example a first setup:

```toml
[[action]]
label = "Set up Jira"
help = "Writes the Jira CLI's configuration: your site, your account and a default project."
command = "jira init --installation cloud --server {server} --login {login} --project {project} --force"
# terminal = true                      # for a command that asks questions: it opens in a terminal

[[action.input]]
name = "server"                        # {server} in the command
label = "Site"
placeholder = "https://example.atlassian.net"

[[action.input]]
name = "project"
label = "Default project key"
default = "ABC"                        # used when the field is left empty; without a default the field must be filled in
```

The command runs in a login shell in the project root, with the sandbox's variables. Each `{name}` is replaced by the field's value, quoted for the shell. Without `terminal`, the command must finish on its own within three minutes; what it prints is shown under the button.

### Settings

Software that is configured through environment variables can describe them, and the sandbox's **Config** tab shows a field for each:

```toml
[[config]]
env = "NPM_CONFIG_REGISTRY"            # the variable
label = "Package registry"             # what the field is called
help = "Where npm downloads packages from."
placeholder = "https://registry.npmjs.org"
options = ["https://registry.npmjs.org"]   # optional: suggestions, or the values of a choice

[[config]]
env = "DISABLE_TELEMETRY"
label = "No usage statistics"
help = "Do not send usage statistics."
kind = "switch"                        # text (default), number, port, switch or choice
on = "1"                               # for a switch: the value that means on
```

A value the user sets is stored with the sandbox and exported in every session; an empty field removes the variable. Only describe variables the software documents. An agent's `model_env` shows up there too, as a field called Model.

For a service that listens on one port, set `port_env` on the recipe and declare the matching `[[config]]` field with `kind = "port"`. Boxquell chooses a free host port on install, exports that same number to the guest, and forwards it from `127.0.0.1`. The Config field can set a specific port or choose another random one; stop the sandbox before changing an active listener. Keep `ports` for any additional fixed forwards.

Rules of thumb:

- List every host the install script contacts in `install_allow`. Anything else is blocked, and `boxquell logs <sandbox> --blocked` shows what was refused.
- Keep `allow` as small as possible: these destinations are open for a sandbox for as long as the recipe is installed. Users see them before the install and can switch each one off under Network.
- The guest is Debian. Install to system paths (`/usr/local`) so every session finds the software.
- An icon is a small single-colour SVG (under 64 KB, no scripts or links); the UI paints it in the text colour. System definitions in `os/` take the same `icon` line.
- An agent recipe can name the command that works through one task without a terminal and then ends: `task = "claude -p {prompt} --dangerously-skip-permissions"`. `{prompt}` is replaced by the task's text, quoted for the shell. The agent must not wait for answers: nobody is there, and the sandbox is what limits it.
- An agent recipe can name the variable that selects its model, `model_env = "ANTHROPIC_MODEL"`; the UI then offers a model field for it.
- Never put credentials into a recipe. Name a secret instead; its value stays on the host.

Recipes run their own scripts inside the sandbox and open the destinations they list. Users are told so when they add a repository, and should add only repositories they trust.
