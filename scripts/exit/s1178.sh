#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Task 1135: the replay of a review that tries a new blocking finding on an untouched line.
set -euo pipefail
cd "$(dirname "$0")/../.."
cargo test -p mason-daemon --test it review_rounds
cargo test -p mason-daemon --lib reviewrounds
