#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s797 exit test: a transient push rejection is retried, an outage is waited out in landing and
# is classified outside the code (#772).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
cargo test -q -p mason-daemon --lib -- remote::tests::a_push_ remote::tests::a_refused_credential
cargo test -q -p mason-daemon --lib -- landing::outage_tests
cargo test -q -p mason-core -- fail_class_tests
echo "s797 passed"
