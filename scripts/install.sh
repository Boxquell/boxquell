#!/bin/sh
# Install the boxquell command on Linux or macOS.
#
#   curl -fsSL https://github.com/Boxquell/boxquell/releases/latest/download/install.sh | sh
#
# It downloads the latest release for this computer, checks it against the
# release's SHA256SUMS and puts `boxquell` into ~/.local/bin. The desktop app
# comes with it: on Linux as an entry in the application menu, on macOS in
# Applications. Nothing needs root. Run it again to update.
#
# Options:
#   --dir <path>     install the command into this directory instead of ~/.local/bin
#   --no-desktop     install the command only, without the desktop app
#   --uninstall      remove the command and the app again (sandboxes and settings stay)
#   --help
# Environment:
#   BOXQUELL_INSTALL_DIR   same as --dir
#   BOXQUELL_RELEASE_URL   folder to download from; default is the latest release
#   NO_COLOR               plain output
set -eu

release_url=${BOXQUELL_RELEASE_URL:-https://github.com/Boxquell/boxquell/releases/latest/download}
install_dir=${BOXQUELL_INSTALL_DIR:-$HOME/.local/bin}
uninstall=0
desktop=1
[ -z "${BOXQUELL_NO_DESKTOP:-}" ] || desktop=0
# Where the desktop app lives on Linux: unpacked, with a menu entry and an icon.
data=${XDG_DATA_HOME:-$HOME/.local/share}
app_dir="$data/boxquell"
menu_entry="$data/applications/boxquell.desktop"
menu_icon="$data/icons/hicolor/256x256/apps/boxquell.png"

# Colors only on a terminal that wants them.
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ] && [ "${TERM:-dumb}" != dumb ]; then
  bold=$(printf '\033[1m') dim=$(printf '\033[2m') blue=$(printf '\033[38;5;39m') green=$(printf '\033[32m')
  yellow=$(printf '\033[33m') red=$(printf '\033[31m') reset=$(printf '\033[0m')
else
  bold='' dim='' blue='' green='' yellow='' red='' reset=''
fi

say() { printf '%s\n' "$*"; }
step() { printf '%s›%s %s\n' "$blue" "$reset" "$*"; }
ok() { printf '%s✓%s %s\n' "$green" "$reset" "$*"; }
note() { printf '%s!%s %s\n' "$yellow" "$reset" "$*"; }
die() { printf '%s✗ %s%s\n' "$red" "$*" "$reset" >&2; exit 1; }

banner() {
  say ""
  say "  ${blue}    ▲${reset}"
  say "  ${blue}   ▟█▙${reset}      ${bold}Boxquell${reset}"
  say "  ${bold} ▛▀▀▀▀▀▜${reset}    ${dim}MicroVM sandboxes for coding agents${reset}"
  say "  ${bold} ▙▄▄▄▄▄▟${reset}"
  say ""
}

usage() {
  cat <<'EOF'
Install the boxquell command on Linux or macOS.

  curl -fsSL https://github.com/Boxquell/boxquell/releases/latest/download/install.sh | sh

Options:
  --dir <path>     install the command into this directory instead of ~/.local/bin
  --no-desktop     install the command only, without the desktop app
  --uninstall      remove the command and the app again (sandboxes and settings stay)
  --help
Environment:
  BOXQUELL_INSTALL_DIR   same as --dir
  BOXQUELL_RELEASE_URL   folder to download from; default is the latest release
  NO_COLOR               plain output
EOF
}

while [ $# -gt 0 ]; do
  case $1 in
    --dir) [ $# -ge 2 ] || die "--dir needs a path"; install_dir=$2; shift ;;
    --uninstall) uninstall=1 ;;
    --no-desktop) desktop=0 ;;
    --help | -h) usage; exit 0 ;;
    *) die "unknown option $1 (see --help)" ;;
  esac
  shift
done

banner

if [ "$uninstall" = 1 ]; then
  if [ -e "$install_dir/boxquell" ]; then
    rm -f "$install_dir/boxquell"
    ok "Removed $install_dir/boxquell"
    say "  ${dim}Your sandboxes and settings were left alone.${reset}"
  else
    note "There is no boxquell in $install_dir."
  fi
  if [ -e "$menu_entry" ] || [ -d "$app_dir/app" ]; then
    rm -rf "$app_dir/app" "$menu_entry" "$menu_icon"
    rmdir "$app_dir" 2>/dev/null || true
    ok "Removed the desktop app and its menu entry"
  fi
  for apps in /Applications "$HOME/Applications"; do
    if [ -d "$apps/Boxquell.app" ]; then
      rm -rf "$apps/Boxquell.app"
      ok "Removed $apps/Boxquell.app"
    fi
  done
  exit 0
fi

# Which build fits this computer.
case $(uname -s) in
  Linux) os=unknown-linux-gnu; system=Linux ;;
  Darwin) os=apple-darwin; system=macOS ;;
  *) die "This installer supports Linux and macOS. On Windows use install.ps1 from the releases page." ;;
esac
case $(uname -m) in
  x86_64 | amd64) cpu=x86_64 ;;
  arm64 | aarch64) cpu=aarch64 ;;
  *) die "Unsupported processor: $(uname -m)" ;;
esac
target="$cpu-$os"

fetch() {
  # fetch <url> <file>
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL --retry 2 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -q -O "$2" "$1"
  else
    die "curl or wget is needed to download boxquell"
  fi
}

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | cut -d' ' -f1
  else
    die "sha256sum or shasum is needed to check the download"
  fi
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT INT TERM

step "Looking up the latest release for $system ($cpu)"
fetch "$release_url/SHA256SUMS" "$work/SHA256SUMS" || die "Could not download the list of release files from $release_url"

# The checksum list names every file of the release; pick the one for this computer.
line=$(grep -E "  boxquell-v?[0-9][^ ]*-$target\.tar\.gz\$" "$work/SHA256SUMS" | head -n 1) || true
[ -n "$line" ] || die "The latest release has no build for $target"
expected=${line%% *}
asset=${line##* }
version=${asset#boxquell-}
version=${version%-"$target".tar.gz}
version=${version#v}

previous=''
if [ -x "$install_dir/boxquell" ]; then
  previous=$("$install_dir/boxquell" --version 2>/dev/null | cut -d' ' -f2) || true
fi

step "Downloading boxquell $version"
fetch "$release_url/$asset" "$work/$asset" || die "Could not download $asset"
actual=$(sha256 "$work/$asset")
[ "$actual" = "$expected" ] || die "The download does not match its checksum (expected $expected, got $actual). Nothing was installed."
ok "Checksum matches"

tar -xzf "$work/$asset" -C "$work"
binary=$(find "$work" -type f -name boxquell | head -n 1)
[ -n "$binary" ] || die "The release archive contains no boxquell program"

mkdir -p "$install_dir"
# Replace in one step, so a running boxquell is never half-written.
cp "$binary" "$install_dir/.boxquell.new"
chmod 755 "$install_dir/.boxquell.new"
mv -f "$install_dir/.boxquell.new" "$install_dir/boxquell"

if [ -n "$previous" ] && [ "$previous" != "$version" ]; then
  ok "Updated boxquell $previous → ${bold}$version${reset} in $install_dir"
else
  ok "Installed boxquell ${bold}$version${reset} to $install_dir"
fi

# The desktop app, when this release has one for this computer. The command
# works without it, so nothing here stops the installation.
app_hint=''
install_desktop() {
  case $system in
    Linux) kind=AppImage ;;
    macOS) kind=app.tar.gz ;;
  esac
  app_line=$(grep -E "  boxquell-desktop-[^ ]*-$target\.$kind\$" "$work/SHA256SUMS" | head -n 1) || true
  if [ -z "$app_line" ]; then
    note "This release has no desktop app for $system ($cpu); the command works without it."
    return 0
  fi
  app_sum=${app_line%% *}
  app_asset=${app_line##* }
  step "Downloading the desktop app"
  if ! fetch "$release_url/$app_asset" "$work/$app_asset"; then
    note "Could not download the desktop app; the command works without it."
    return 0
  fi
  [ "$(sha256 "$work/$app_asset")" = "$app_sum" ] || die "The desktop app does not match its checksum. It was not installed."

  if [ "$system" = Linux ]; then
    # Unpacked once, so starting it needs no FUSE.
    chmod +x "$work/$app_asset"
    if ! (cd "$work" && "./$app_asset" --appimage-extract >/dev/null 2>&1) || [ ! -x "$work/squashfs-root/AppRun" ]; then
      note "Could not unpack the desktop app; the command works without it."
      return 0
    fi
    mkdir -p "$app_dir" "$(dirname "$menu_entry")" "$(dirname "$menu_icon")"
    rm -rf "$app_dir/app"
    mv "$work/squashfs-root" "$app_dir/app"
    cp "$app_dir/app/Boxquell.png" "$menu_icon" 2>/dev/null || true
    cat > "$menu_entry" <<DESKTOP
[Desktop Entry]
Type=Application
Name=Boxquell
Comment=MicroVM sandboxes for coding agents
Exec=env "BOXQUELL_BIN=$install_dir/boxquell" "$app_dir/app/AppRun"
Icon=boxquell
Terminal=false
Categories=Development;
StartupWMClass=boxquell-desktop
DESKTOP
    if command -v update-desktop-database >/dev/null 2>&1; then
      update-desktop-database "$data/applications" >/dev/null 2>&1 || true
    fi
    ok "Installed the desktop app; ${bold}Boxquell${reset} is in your application menu"
    app_hint="Open ${bold}Boxquell${reset} from your application menu, or use the command:"
  else
    apps=/Applications
    [ -w "$apps" ] || apps="$HOME/Applications"
    mkdir -p "$apps" "$work/app"
    tar -xzf "$work/$app_asset" -C "$work/app"
    [ -d "$work/app/Boxquell.app" ] || { note "The desktop app's archive is not as expected; the command works without it."; return 0; }
    rm -rf "$apps/Boxquell.app"
    mv "$work/app/Boxquell.app" "$apps/Boxquell.app"
    ok "Installed the desktop app to $apps/Boxquell.app"
    app_hint="Open ${bold}Boxquell${reset} from Applications or Spotlight, or use the command:"
  fi
}
[ "$desktop" = 0 ] || install_desktop

# What boxquell needs around it. Nothing here is installed for you.
say ""
say "${bold}What boxquell needs${reset}"
have() { command -v "$1" >/dev/null 2>&1; }
found_lib() {
  for dir in /usr/lib /usr/lib64 /usr/local/lib /opt/homebrew/lib /usr/lib/x86_64-linux-gnu /usr/lib/aarch64-linux-gnu; do
    ls "$dir"/libkrun.* >/dev/null 2>&1 && return 0
  done
  return 1
}
missing=0
check() {
  # check <0 when present> <name> <how to get it>
  if [ "$1" = 0 ]; then
    ok "$2"
  else
    note "$2 is missing  ${dim}$3${reset}"
    missing=$((missing + 1))
  fi
}
result() { if "$@"; then echo 0; else echo 1; fi; }
has_kvm() { [ -r /dev/kvm ] && [ -w /dev/kvm ]; }
has_runner() { found_lib || have qemu-system-x86_64; }

if [ "$system" = macOS ]; then
  check "$(result found_lib)" "libkrun (runs the sandboxes)" "brew tap slp/krun && brew install libkrun"
else
  check "$(result has_runner)" "libkrun or QEMU (runs the sandboxes)" "install libkrun and libkrunfw, or qemu-system-x86_64"
  check "$(result has_kvm)" "KVM (/dev/kvm)" "enable virtualization and join the kvm group"
fi
check "$(result have ssh)" "OpenSSH client" "install openssh"

say ""
say "${bold}Next${reset}"
case ":$PATH:" in
  *":$install_dir:"*) cmd=boxquell ;;
  *)
    cmd="$install_dir/boxquell"
    note "$install_dir is not on your PATH. Add it to use plain ${bold}boxquell${reset}:"
    say "    ${dim}export PATH=\"$install_dir:\$PATH\"${reset}"
    ;;
esac
if [ "$missing" -gt 0 ]; then
  say "  Install what is marked above, then:"
fi
[ -z "$app_hint" ] || say "  $app_hint"
say "  ${blue}$cmd doctor${reset}                 ${dim}checks that everything is in place${reset}"
say "  ${blue}$cmd new mybox --workspace .${reset}  ${dim}a sandbox for the project in this folder;${reset}"
say "                                    ${dim}the system is downloaded on first use${reset}"
say "  ${blue}$cmd shell mybox${reset}"
say ""
say "  ${dim}Docs: https://github.com/Boxquell/boxquell${reset}"
say ""
