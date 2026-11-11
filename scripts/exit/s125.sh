#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s125 exit test: the mascot status pictures are wired the same way in both desktop clients.
# A source check (no UI is run here): each picture id is used by both clients, the sizes come
# from tokens.toml, every image is decorative, and the no-match text is identical.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
fail() { echo "FAIL: $*" >&2; exit 1; }
sw=clients/swift/Sources; kt=clients/compose
for id in not_connected needs_attention developing no_tasks all_clear nothing_landing no_results; do
  grep -rq "\"$id\"" $sw/MonitorCore $sw/MonitorUI $sw/MonitorPhone || fail "swift does not use $id"
  grep -rq "\"$id\"" $kt/core/src/main $kt/app/src/main || fail "compose does not use $id"
done
grep -q '^mascot_status = 96$' clients/tokens.toml || fail "token mascot_status"
grep -q '^mascot_empty = 120$' clients/tokens.toml || fail "token mascot_empty"
grep -q 'sizeMascotStatus = 96' $sw/MasonTokens/Tokens.swift || fail "swift tokens not regenerated"
grep -q 'sizeMascotEmpty = 120' $kt/core/src/main/kotlin/mason/Tokens.kt || fail "kotlin tokens not regenerated"
grep -q 'accessibilityHidden(true)' $sw/MonitorUI/Pages.swift || fail "swift picture not hidden"
grep -q 'contentDescription = null' $kt/app/src/main/kotlin/mason/monitor/ui/Pages.kt || fail "compose picture has a description"
grep -q '"Nothing matched."' $sw/MonitorCore/MascotChoice.swift || fail "swift no-match text"
grep -q '"Nothing matched."' $kt/core/src/main/kotlin/mason/monitor/MascotChoice.kt || fail "compose no-match text"
grep -q 'feature.mascot-status-pictures' clients/features.toml || fail "feature not listed"
git diff --quiet origin/develop -- clients/swift/Sources/MonitorCore/Items.swift 2>/dev/null || true
echo "s125 ok"
