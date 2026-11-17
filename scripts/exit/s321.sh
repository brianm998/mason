#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s321 exit test: views are stamped with their project and none for the old filter follows the new.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q "string project = 10;" schemas/mason.proto
cargo test -q -p mason-daemon --test it views_are_stamped_and_none_for_the_old_project
echo ok
