#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s122 exit test: the start-image token is in all three places and the feature is listed.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q 'mascot_start_width' clients/tokens.toml
grep -q 'sizeMascotStartWidth' clients/swift/Sources/MasonTokens/Tokens.swift
grep -q 'sizeMascotStartWidth' clients/compose/core/src/main/kotlin/mason/Tokens.kt
grep -q 'feature.mascot-start-image' clients/features.toml
echo "s122: ok"
