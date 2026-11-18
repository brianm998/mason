#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s484 exit test: the APNs token cache is keyed on the key.
set -euo pipefail
cd "$(dirname "$0")/../.."
out=$(mktemp /tmp/s484.XXXXXX)
trap 'rm -f "$out"' EXIT
cargo test -p mason-daemon --test it apns:: 2>&1 | tee "$out"
grep -q 'apns::a_push_reaches_owner_devices_with_a_jwt_that_verifies_and_skips_viewers ... ok' "$out"
cargo test -p mason-adapter-api apns
