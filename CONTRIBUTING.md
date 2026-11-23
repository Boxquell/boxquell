# Contributing

Thanks for helping. A few things that make a change easy to accept:

- **Talk first for bigger changes.** Open an issue describing the problem before writing a large patch.
- **Recipes** (agents, languages, tools) live in their own repositories: [agents](https://github.com/Boxquell/boxquell-agents), [languages](https://github.com/Boxquell/boxquell-languages), [tools](https://github.com/Boxquell/boxquell-tools). The format is in [docs/recipes.md](docs/recipes.md).
- **Security problems** go through the private report in [SECURITY.md](SECURITY.md), not a public issue.

## Working on the code

```sh
(cd ui && npm ci && npm run build)   # the web UI is embedded into the binary
cargo build
scripts/boxquell-dev doctor          # runs the debug build with ./.boxquell-dev as data directory
```

Before opening a pull request:

```sh
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Changes to the gateway, the VM backends or the guest scripts in `image/` need a run in a real sandbox; the tests cannot boot one. Say in the pull request what you tried.

After changing dependencies, run `scripts/third-party-notices.sh`.

By contributing you agree that your work is licensed under MIT OR Apache-2.0, like the rest of the project.
