#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Task 180 exit test: the weakening scan skips closures, counters and cleanup idioms, and
# runs at submit_change, with the stand-in agent and a real masond (`[self] holds = "review"`).
#  1. A change adding `&|| true` in Rust and `grep -c ... || true` to an existing script lands
#     without a hold.
#  2. A change adding `cargo test || true` to an existing script is refused at submit_change,
#     and the refusal names the file and line.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s166.XXXXXX); daemon=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  for hd in "$root"/d/h; do target/debug/step-host-ctl reap "$hd" >/dev/null 2>&1 || true; done
  rm -rf "$root"
}
trap cleanup EXIT

cargo build -q --workspace --bins
bin=target/debug
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
jq_state() { sed -n 's/^  "state": "\(.*\)",$/\1/p'; }
wait_state() {  # TASK STATE SECONDS
  local end=$((SECONDS + $3))
  until [ "$(m task show "$1" --json 2>/dev/null | jq_state)" = "$2" ]; do
    [ $SECONDS -lt $end ] || { echo "timed out waiting for task $1 to be $2" >&2; m task show "$1" >&2 || true; exit 1; }
    sleep 0.3
  done
}

git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
mkdir -p "$root/repo/scripts" "$root/repo/src"
printf '#!/usr/bin/env bash\necho gate\n' > "$root/repo/scripts/gate.sh"
printf '#!/usr/bin/env bash\necho other\n' > "$root/repo/scripts/other.sh"
echo 'fn main() {}' > "$root/repo/src/main.rs"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm init
git -C "$root/repo" push -q origin main
git -C "$root/repo" checkout -q -b owner-work

cat > "$root/agent.sh" <<'AGENT'
g() { git -c user.name=t -c user.email=t@t "$@"; }
case $(basename "$PWD") in
  0) echo 'run(&|| true, 1);' >> src/main.rs
     echo 'n=$(grep -c gate scripts/gate.sh || true)' >> scripts/gate.sh ;;
  1) echo 'cargo test || true' >> scripts/other.sh ;;
esac
g add -A; g commit -qm change --allow-empty
AGENT
mkdir -p "$root/c/projects"
turn="{\"sh\": \"bash $root/agent.sh\"}, {\"call\": {\"tool\": \"submit_change\", \"args\": {\"summary\": \"done\", \"head\": \"\$HEAD\"}}}"
echo "{\"steps\": {\"develop\": [[$turn]]}}" > "$root/script.json"
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"
claude_bin = "$PWD/$bin/fake-claude"
[self]
holds = "review"
max_open = 5
[agent_env]
FAKE_CLAUDE_SCRIPT = "$root/script.json"
TOML
"$bin/masond" --port 0 --data-dir "$root/d" --config-dir "$root/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 4 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done

echo "== 1. \`&|| true\` in Rust and \`grep -c ... || true\` in a script land without a hold"
m task add "harmless idioms" --project mason --explored --body "x" >/dev/null   # 0
wait_state 0 landed 90
git -C "$root/origin.git" show main:src/main.rs | grep -q '&|| true'
echo "task #0 landed"

echo "== 2. \`cargo test || true\` in an existing script is refused at submit_change"
m task add "weaken the gate" --project mason --explored --body "x" >/dev/null   # 1
end=$((SECONDS + 90))
until grep -rqs 'scripts/other.sh:3: broadened-skip' "$root/d"; do
  [ $SECONDS -lt $end ] || { echo "no refusal naming scripts/other.sh:3" >&2; exit 1; }
  sleep 0.3
done
grep -rhs -o 'scripts/other.sh:3: broadened-skip[^"]*' "$root/d" | sed -n 1p
if git -C "$root/origin.git" show main:scripts/other.sh | grep -q 'cargo test || true'; then
  echo "the weakening landed" >&2; exit 1
fi

trap - EXIT; cleanup
echo "s166 OK"
