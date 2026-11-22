#!/bin/sh
# SPDX-License-Identifier: Apache-2.0
set -e
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --test it -- testing::three_debts testing::after_a_red testing::a_ci_run
