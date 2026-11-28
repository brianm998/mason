#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1232 exit test: the settings catalogue (decision 1222). The tests start a scratch daemon with
# a fixture project, read every section, set and unset a tier mapping and check the next launch,
# and check that a literal token, a fixed key, a viewer's write and a paired device's write of a
# program key are each refused with the file unchanged.
set -euo pipefail
cd "$(dirname "$0")/../.."

grep -q '^Summary: ' docs/stages/s1232.md
cargo test -q -p mason-daemon --test it projsettings::
echo "OK s1232"
