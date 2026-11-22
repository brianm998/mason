#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
set -e
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --test it -- testing::three_debts testing::after_a_red testing::a_ci_run
