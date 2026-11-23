#!/bin/sh
# Print the changelog entries of one version, without the heading.
# Falls back to the Unreleased section, which is what a fresh tag describes.
#   scripts/changelog-section.sh 1.2.3
set -eu
version=${1:?usage: changelog-section.sh <version>}
file=${2:-$(dirname "$0")/../CHANGELOG.md}
section() {
  awk -v want="$1" '
    /^## / { on = (index($0, "## [" want "]") == 1); next }
    /^\[[^]]+\]: / { next }
    on { print }' "$file"
}
text=$(section "$version")
[ -n "$(printf '%s' "$text" | tr -d '[:space:]')" ] || text=$(section "Unreleased")
printf '%s\n' "$text" | sed -e '/./,$!d'
