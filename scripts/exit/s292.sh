#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s292 exit test: a project whose [tracker] is a plugin imports an issue through it, a suite's
# results come from a parser plugin, and the adapter conformance kits pass against plugin
# adapters. Plugins are built only from approved hashes.
set -euo pipefail
cd "$(dirname "$0")/../.."
# [tracker] kind = "plugin:<name>" parses; the tracker lists an issue; tracker_kit, host_kit and
# build_kit pass; the parser plugin's results give the verdict; an unapproved plugin builds nothing.
cargo test -q -p mason-daemon --test it plugin_adapters::
cargo test -q -p mason-daemon --lib -- host_ci_and_tracker_can_be_a_plugin
cargo test -q -p mason-plugins --test adapters
cargo test -q -p mason-testing --test plugin_results
echo "s292 OK"
