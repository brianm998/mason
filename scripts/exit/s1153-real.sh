#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1153 against the real `claude` CLI: the init event of a session given a plugin directory and
# the settings MASON writes lists the plugin's skill and not the working directory's.
# MANUAL: never run by CI. It spends no tokens: the API address is a closed port on this machine,
# HOME is a temporary directory, and the CLI is stopped as soon as it has said what it loaded.
#
#   MASON_REAL_APPROVED=yes     you have read this script and approve running it
set -euo pipefail
[ "${MASON_REAL_APPROVED:-}" = yes ] || { echo "refusing: set MASON_REAL_APPROVED=yes once you approve running this" >&2; exit 2; }
claude=$(command -v claude) || { echo "needs the real claude CLI" >&2; exit 2; }
root=$(mktemp -d /tmp/mason-s1153r.XXXXXX)
pid=
trap '[ -z "$pid" ] || kill "$pid" 2>/dev/null || true; rm -rf "$root"' EXIT

skill() {
    mkdir -p "$1"
    printf -- '---\nname: %s\ndescription: A fixture.\n---\nbody\n' "${1##*/}" > "$1/SKILL.md"
}
skill "$root/plugin/skills/fixture"
mkdir -p "$root/plugin/.claude-plugin" "$root/home" "$root/wt"
echo '{"name":"mason","version":"1.0.0"}' > "$root/plugin/.claude-plugin/plugin.json"
git init -q "$root/wt"
skill "$root/wt/.claude/skills/second"

(cd "$root/wt" && echo hi | exec env -i PATH="$PATH" HOME="$root/home" \
    ANTHROPIC_BASE_URL=http://127.0.0.1:1 ANTHROPIC_API_KEY=none \
    HTTPS_PROXY=http://127.0.0.1:1 CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC=1 \
    DISABLE_AUTOUPDATER=1 NO_PROXY=127.0.0.1 \
    "$claude" -p --verbose --output-format stream-json --strict-mcp-config \
    --plugin-dir "$root/plugin" --tools Read,Skill --allowedTools Read Skill \
    --permission-mode dontAsk --settings \
    '{"disableBundledSkills":true,"skillOverrides":{"second":"off","doctor":"off","plugin-authoring":"off"}}' \
    > "$root/out.jsonl") &
pid=$!
for _ in $(seq 1 600); do
    grep -q '"subtype":"init"' "$root/out.jsonl" 2>/dev/null && break
    sleep 0.1
done
listed=$(grep -m1 '"subtype":"init"' "$root/out.jsonl" | sed 's/.*"skills":\[\([^]]*\)\].*/\1/')
echo "skills: $listed"
[ "$listed" = '"mason:fixture"' ] || { echo "FAILED: expected only mason:fixture" >&2; exit 1; }
echo "OK s1153-real"
