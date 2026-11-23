#!/usr/bin/env bash
# Regenerate THIRD-PARTY-NOTICES.md from the Rust and npm dependencies.
# Needs cargo-about (`cargo install cargo-about`) and an installed ui/node_modules.
set -euo pipefail
cd "$(dirname "$0")/.."
out=THIRD-PARTY-NOTICES.md
{
  cat <<'HEAD'
# Third-party notices

Boxquell is built from the open-source components listed here. Their licenses
apply to the respective parts; boxquell's own license is in `LICENSE-MIT` and
`LICENSE-APACHE`.

## Components used at run time, not distributed with boxquell

- **libkrun** and **libkrunfw** (https://github.com/containers/libkrun): Apache-2.0; libkrunfw
  bundles a Linux kernel, GPL-2.0-only. Loaded from the system at run time.
- **QEMU** (https://www.qemu.org): GPL-2.0-or-later. Started as a separate program.
- **Debian** (https://www.debian.org): the guest image is built on your computer from
  Debian packages, each under its own license (see `/usr/share/doc/*/copyright` in the guest).
- **OpenSSH**, **git** and **e2fsprogs** on the host are run as separate programs.

## Web UI (npm packages bundled into the binary)

HEAD
  node - <<'NODE'
const fs = require("fs"), path = require("path");
const root = "ui/node_modules";
const seen = new Set();
const walk = (name) => {
  if (seen.has(name)) return;
  seen.add(name);
  const pkg = JSON.parse(fs.readFileSync(path.join(root, name, "package.json"), "utf8"));
  Object.keys(pkg.dependencies ?? {}).forEach(walk);
};
Object.keys(JSON.parse(fs.readFileSync("ui/package.json", "utf8")).dependencies ?? {}).forEach(walk);
for (const name of [...seen].sort()) {
  const dir = path.join(root, name);
  const pkg = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));
  const file = fs.readdirSync(dir).find((f) => /^licen[sc]e/i.test(f));
  const repo = typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url ?? "";
  console.log(`### ${name} ${pkg.version}\n\nLicense: ${pkg.license}${repo ? ` (${repo})` : ""}\n`);
  if (file) console.log("```\n" + fs.readFileSync(path.join(dir, file), "utf8").trim() + "\n```\n");
}
NODE
  cargo about generate about.hbs
} > "$out"
echo "wrote $out ($(wc -c < "$out") bytes)"
