#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Print the version for a release tag `vX.Y.Z`; fail when the tag is malformed or is not
# the workspace version in Cargo.toml. The release workflow calls this first.
# Usage: release-version.sh <tag>
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
tag="${1:-}"
if [ -z "$tag" ]; then
  echo "usage: release-version.sh vX.Y.Z" >&2; exit 2
fi
if ! [[ "$tag" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]]; then
  echo "release-version: '$tag' is not a release tag (want vX.Y.Z)" >&2; exit 1
fi
ver="${BASH_REMATCH[1]}"
have=$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version *=/{gsub(/[" ]/,"",$0); sub(/^version=/,"",$0); print; exit}' "$root/Cargo.toml")
if [ -z "$have" ]; then
  echo "release-version: no workspace version in Cargo.toml" >&2; exit 1
fi
if [ "$ver" != "$have" ]; then
  echo "release-version: tag $tag is version $ver but Cargo.toml says $have; bump the version first" >&2; exit 1
fi
echo "$ver"
