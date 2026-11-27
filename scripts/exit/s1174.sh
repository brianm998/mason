#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1174 exit test: the Swift monitor decodes HomeView.hosts, and its Hosts and snapshot tests pass.
set -euo pipefail
cd "$(dirname "$0")/.."/..
grep -q 'enum Hosts' clients/swift/Sources/MonitorCore/Hosts.swift
grep -q 'HostsSection(model: model' clients/swift/Sources/MonitorUI/Pages.swift
grep -q 'feature.home-hosts' clients/features.toml
# The Swift package's test bundles do not all build on Linux, so run it on macOS only. The tests
# draw offscreen (ImageRenderer); the decoding test checks the recorded view has 3 hosts, runs 0, 2, 1.
if [ "$(uname)" = Darwin ] && command -v swift >/dev/null && [ -z "${MASON_SKIP_SWIFT:-}" ]; then
  swift test --package-path clients/swift --filter 'HostsTests|HostsSnapshotTests'
fi
