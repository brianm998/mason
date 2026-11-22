#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# The test step's clippy suite (s778): `cargo clippy <packages> --all-targets -- -D warnings`
# reduced to one line, `test clippy_clean ... ok|FAILED`, with the first warnings printed before it.
# Arguments are cargo's package flags (`--workspace`, or `-p a -p b`).
cd "$(dirname "$0")/.." || exit 1
out=$(mktemp)
trap 'rm -f "$out"' EXIT
if cargo clippy "$@" --all-targets -- -D warnings >"$out" 2>&1; then
    echo "test clippy_clean ... ok"
else
    grep -E -A6 '^(warning|error)' "$out" | head -60
    echo "test clippy_clean ... FAILED"
    exit 1
fi
