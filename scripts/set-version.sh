#!/bin/sh
# Write a version into every place that carries one. The release workflow
# runs this before building, so a tag decides the version, not a commit.
#   scripts/set-version.sh 1.2.3
set -eu
version=${1:?usage: set-version.sh <version>}
printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$' || { echo "error: invalid version $version" >&2; exit 1; }
cd "$(dirname "$0")/.."

# Portable in-place edit: BSD and GNU sed disagree about -i.
edit() { sed "$1" "$2" > "$2.tmp" && mv "$2.tmp" "$2"; }

edit "s/^version = \"[^\"]*\"$/version = \"$version\"/" Cargo.toml
edit "s/^version = \"[^\"]*\"$/version = \"$version\"/" apps/desktop/src-tauri/Cargo.toml
for file in ui/package.json apps/desktop/package.json apps/desktop/src-tauri/tauri.conf.json; do
  edit "s/^  \"version\": \"[^\"]*\",$/  \"version\": \"$version\",/" "$file"
done
# The lock files name the workspace crates with their version.
for crate in boxquell boxquell-core boxquell-net boxquell-vm; do
  awk -v crate="$crate" -v version="$version" '
    $0 == "name = \"" crate "\"" { print; hit = 1; next }
    hit && /^version = / { print "version = \"" version "\""; hit = 0; next }
    { hit = 0; print }' Cargo.lock > Cargo.lock.tmp && mv Cargo.lock.tmp Cargo.lock
done
echo "version set to $version"
