#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s446 exit test: the universal iOS app builds for the simulator with `make ios`.
# Skips (exit 0, with the reason printed) where Xcode or XcodeGen is missing.
set -euo pipefail
cd "$(dirname "$0")/../../clients/swift"
if ! command -v xcodebuild >/dev/null 2>&1 || ! xcodebuild -version >/dev/null 2>&1; then
  echo "s446 SKIP: no Xcode here, so the iOS app was not built"
  exit 0
fi
if ! command -v xcodegen >/dev/null 2>&1; then
  echo "s446 SKIP: XcodeGen is not installed (brew install xcodegen), so the iOS app was not built"
  exit 0
fi
# Reuse the package checkouts SwiftPM already has, so the build does not need the network.
settings=""
if [ -d .build/checkouts ] && [ -f .build/workspace-state.json ]; then
  pk="$(mktemp -d)"
  trap 'rm -rf "$pk"' EXIT
  cp -R .build/checkouts "$pk/checkouts"
  [ -d .build/repositories ] && cp -R .build/repositories "$pk/repositories"
  cp .build/workspace-state.json "$pk/"
  settings="-clonedSourcePackagesDirPath $pk"
fi
make ios IOS_SETTINGS="$settings"
echo "s446 OK"
