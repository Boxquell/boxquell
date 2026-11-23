#!/bin/sh
# After a release: give its changelog section the version and date, link it,
# and open a fresh Unreleased section.
#   scripts/finalise-changelog.sh 1.2.3 2026-11-23
set -eu
version=${1:?usage: finalise-changelog.sh <version> <date>}
date=${2:?usage: finalise-changelog.sh <version> <date>}
file=${3:-$(dirname "$0")/../CHANGELOG.md}
repo=https://github.com/Boxquell/boxquell

if grep -q "^## \[$version\] - [0-9]" "$file"; then
  echo "changelog already has $version"
  exit 0
fi
awk -v version="$version" -v date="$date" '
  # A section prepared under the version, still marked as unreleased.
  index($0, "## [" version "] - Unreleased") == 1 { print "## [Unreleased]\n\n## [" version "] - " date; done = 1; next }
  # Otherwise everything collected under Unreleased becomes this version.
  !done && $0 == "## [Unreleased]" { print "## [Unreleased]\n\n## [" version "] - " date; done = 1; next }
  { print }' "$file" > "$file.tmp" && mv "$file.tmp" "$file"
grep -q "^\[$version\]: " "$file" || printf '[%s]: %s/releases/tag/v%s\n' "$version" "$repo" "$version" >> "$file"
echo "changelog finalised for $version ($date)"
