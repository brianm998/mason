#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s124 exit test: the state illustration token and feature are in place.
set -euo pipefail
cd "$(dirname "$0")/../.."
grep -q '^mascot_illustration = 96' clients/tokens.toml
grep -q 'sizeMascotIllustration = 96' clients/swift/Sources/MasonTokens/Tokens.swift
grep -q 'sizeMascotIllustration = 96' clients/compose/core/src/main/kotlin/mason/Tokens.kt
grep -q 'feature.mascot-state-illustration' clients/features.toml
echo "s124 OK"
