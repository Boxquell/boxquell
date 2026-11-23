#!/bin/sh
# Check a release tag and print its version.
#   scripts/release-tag.sh v1.2.3        -> 1.2.3
#   scripts/release-tag.sh v1.2.3-rc.1   -> 1.2.3-rc.1
# A final release must be tagged on a commit that is on origin/main.
set -eu
tag=${1:?usage: release-tag.sh <tag>}
version=${tag#v}
if ! printf '%s\n' "$tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$'; then
  echo "error: $tag is not a release tag; expected vX.Y.Z or vX.Y.Z-rc.N" >&2
  exit 1
fi
if [ "${RELEASE_TAG_SKIP_BRANCH_CHECK:-}" != 1 ]; then
  commit=$(git rev-parse "$tag^{commit}")
  if ! git merge-base --is-ancestor "$commit" origin/main; then
    echo "error: $tag is not on origin/main; releases are cut from main" >&2
    exit 1
  fi
fi
echo "$version"
