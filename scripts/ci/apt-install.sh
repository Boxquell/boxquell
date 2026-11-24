#!/usr/bin/env bash
# Install Ubuntu packages in CI with bounded retries and an optional .deb cache.
# Usage: bash scripts/ci/apt-install.sh [--cache-dir DIR] PACKAGE...
set -Eeuo pipefail

archive_cache=""
while [[ "${1:-}" == --* ]]; do
  case "$1" in
    --cache-dir)
      if [[ $# -lt 2 ]]; then
        echo "missing path after --cache-dir" >&2
        exit 2
      fi
      archive_cache=$2
      shift 2
      ;;
    *)
      echo "unsupported option: $1" >&2
      exit 2
      ;;
  esac
done
if [[ $# -eq 0 ]]; then
  echo "provide at least one package name" >&2
  exit 2
fi

case "$(dpkg --print-architecture)" in
  amd64)
    mirrors=(http://archive.ubuntu.com/ubuntu http://mirrors.edge.kernel.org/ubuntu)
    ;;
  arm64)
    mirrors=(http://ports.ubuntu.com/ubuntu-ports http://mirrors.edge.kernel.org/ubuntu-ports)
    ;;
  *)
    echo "unsupported Ubuntu architecture: $(dpkg --print-architecture)" >&2
    exit 2
    ;;
esac

printf '%s\n' \
  'Acquire::Retries "3";' \
  'Acquire::http::Timeout "15";' \
  'Acquire::https::Timeout "15";' \
  'Acquire::ForceIPv4 "true";' \
  | sudo tee /etc/apt/apt.conf.d/99boxquell-ci-downloads >/dev/null

apt_options=(-y -q --no-install-recommends)
if [[ -n "$archive_cache" ]]; then
  mkdir -p "$archive_cache/partial"
  apt_options+=(-o "Dir::Cache::Archives=$archive_cache")
fi

point_ubuntu_sources_at() {
  local base_url=$1
  local source
  for source in /etc/apt/sources.list.d/ubuntu.sources /etc/apt/sources.list; do
    [[ -f "$source" ]] || continue
    sudo sed -i -E \
      -e "s#https?://([[:alnum:].-]+\\.)?(archive|ports)\\.ubuntu\\.com/ubuntu(-ports)?/?#${base_url}/#g" \
      -e "s#https?://mirrors\\.edge\\.kernel\\.org/ubuntu(-ports)?/?#${base_url}/#g" \
      "$source"
  done
}

clear_interrupted_apt_state() {
  sudo pkill -x apt-get >/dev/null 2>&1 || true
  sudo rm -f /var/lib/apt/lists/lock /var/cache/apt/archives/lock
  sudo dpkg --configure -a >/dev/null 2>&1 || true
}

install_from_current_sources() {
  sudo timeout --kill-after=10s 180s apt-get update -q &&
    sudo timeout --kill-after=10s 480s apt-get install "${apt_options[@]}" "$@"
}

# Try the runner's configured mirror first, then two regional Ubuntu mirrors.
for attempt in 0 1 2; do
  if install_from_current_sources "$@"; then
    exit 0
  fi

  if (( attempt == 2 )); then
    echo "apt: all configured Ubuntu mirrors failed or timed out" >&2
    exit 1
  fi

  clear_interrupted_apt_state
  mirror=${mirrors[$attempt]}
  echo "apt: retrying with $mirror after attempt $((attempt + 1))" >&2
  point_ubuntu_sources_at "$mirror"
done
