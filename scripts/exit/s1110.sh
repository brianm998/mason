#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1110 exit test: every action is pinned to a commit SHA, check-action-pins.sh fails on a tag
# pin naming the line, CI's lint job runs it and Dependabot moves the pins.
set -euo pipefail
cd "$(dirname "$0")/../.."
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

fail() { echo "FAILED: s1110: $*" >&2; exit 1; }

# A tag pin fails and the line is named; a local action and a SHA pin pass.
sha=0123456789abcdef0123456789abcdef01234567
printf 'steps:\n  - uses: ./.github/actions/x\n  - uses: foo/bar@%s # v1.0.0\n' "$sha" >"$tmp/good.yml"
bash scripts/check-action-pins.sh "$tmp/good.yml" || fail "a pinned file was rejected"
for ref in v1 master "${sha%?}"; do
  printf 'steps:\n  - uses: foo/bar@%s\n' "$ref" >"$tmp/bad.yml"
  if out=$(bash scripts/check-action-pins.sh "$tmp/bad.yml" 2>&1); then fail "foo/bar@$ref passed"; fi
  case "$out" in
    *"bad.yml:2: not pinned"*"foo/bar@$ref"*) ;;
    *) fail "the line was not named: $out" ;;
  esac
done

# A remote run before this change lands carries the base branch's workflows (the trust overlay),
# which have neither the pins nor the lint step; only the composite actions are this tree's then.
if [ -f .mason/run.json ] && ! grep -q 'check-action-pins' .github/workflows/ci.yml; then
  echo "s1110: base branch workflows in this snapshot; checking composite actions only"
  bash scripts/check-action-pins.sh .github/actions/*/action.yml
else
  bash scripts/check-action-pins.sh
  # One real workflow with one pin turned back into a tag.
  sed 's|actions/checkout@[0-9a-f]*|actions/checkout@v4|' .github/workflows/pages.yml >"$tmp/pages.yml"
  if bash scripts/check-action-pins.sh "$tmp/pages.yml" 2>/dev/null; then fail "a tag pin in a workflow copy passed"; fi
  grep -qF -- '- run: bash scripts/check-action-pins.sh' .github/workflows/ci.yml || fail "CI does not run the check"
fi
grep -qF 'package-ecosystem: github-actions' .github/dependabot.yml || fail "no Dependabot block"
echo "s1110 OK"
