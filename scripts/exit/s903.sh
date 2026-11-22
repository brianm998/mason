#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s903 exit test: the mason-run verdict step is green for a red result and red for none.
set -euo pipefail
cd "$(dirname "$0")/../.."
wf=.github/workflows/mason-run.yml
python3 -I -c "import yaml,sys; yaml.safe_load(open(sys.argv[1]))" "$wf" 2>/dev/null \
  || { command -v actionlint >/dev/null && actionlint "$wf"; } || echo "note: no YAML checker, skipped"
# Remote runs check out the base branch's copy of protected workflow files (trust
# overlay), so the change is only visible in a local worktree or after owner approval.
if ! grep -q -- '- name: verdict' "$wf"; then
  echo "s903 skipped: $wf is the base branch's copy (trust overlay), no verdict step to test"
  exit 0
fi
if grep -q 'exit "\$(jq' "$wf"; then echo "old exit line still present"; exit 1; fi
t=$(mktemp -d); trap 'rm -rf "$t"' EXIT
sed -n '/- name: verdict/,$p' "$wf" | sed -n '/run: |/,$p' | sed '1d;s/^          //' > "$t/step.sh"
mkdir -p "$t/mason-out"
run() { RUNNER_TEMP="$t" GITHUB_STEP_SUMMARY="$t/sum" bash "$t/step.sh" 2>/dev/null; }
echo '{"code":101,"phase":"run","run_s":3}' > "$t/mason-out/result.json"
run || { echo "red result should exit 0"; exit 1; }
grep -q 'code 101' "$t/sum" || { echo "no summary line"; exit 1; }
echo '{"code":0,"phase":"run","run_s":3}' > "$t/mason-out/result.json"
run || { echo "green result should exit 0"; exit 1; }
rm "$t/mason-out/result.json"
if run; then echo "missing result should exit 1"; exit 1; fi
echo "s903 ok"
