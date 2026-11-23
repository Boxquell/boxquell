#!/bin/sh
# Put the Boxquell desktop app into the application menu of this Linux user,
# pointing at the builds in this checkout. Packaged releases do this themselves.
#   scripts/install-desktop-entry.sh            install
#   scripts/install-desktop-entry.sh --remove   take it out again
set -eu
root=$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)
data=${XDG_DATA_HOME:-$HOME/.local/share}
entry="$data/applications/boxquell.desktop"
icon="$data/icons/hicolor/256x256/apps/boxquell.png"

if [ "${1:-}" = "--remove" ]; then
  rm -f "$entry" "$icon"
  echo "removed the Boxquell menu entry"
  exit 0
fi

app="$root/apps/desktop/src-tauri/target/release/boxquell-desktop"
cli="$root/target/release/boxquell"
[ -x "$cli" ] || cli="$root/target/debug/boxquell"
[ -x "$app" ] || { echo "build the app first: (cd apps/desktop && npm install && npx tauri build --no-bundle)" >&2; exit 1; }
[ -x "$cli" ] || { echo "build boxquell first: cargo build --release" >&2; exit 1; }

# A checkout keeps its sandboxes in .boxquell-dev when that directory exists.
env="BOXQUELL_BIN=$cli"
[ -d "$root/.boxquell-dev" ] && env="$env BOXQUELL_HOME=$root/.boxquell-dev"
[ -f "$root/.vendor/krun/usr/lib/libkrun.so.1" ] && env="$env BOXQUELL_KRUN_LIB=$root/.vendor/krun/usr/lib/libkrun.so.1"

mkdir -p "$(dirname "$entry")" "$(dirname "$icon")"
cp "$root/assets/icon-256.png" "$icon"
cat > "$entry" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Boxquell
Comment=MicroVM sandboxes for coding agents
Exec=env $env "$app"
Icon=boxquell
Terminal=false
Categories=Development;
StartupWMClass=boxquell-desktop
DESKTOP
command -v update-desktop-database >/dev/null && update-desktop-database "$data/applications" >/dev/null 2>&1 || true
echo "installed $entry"
