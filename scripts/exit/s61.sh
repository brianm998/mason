#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Tracker setup (s61) exit test. A real masond, a fake GitHub (fake-github, over recorded JSON in
# scripts/exit/fixtures/s61; loopback only) and bare repositories. No network.
#  1. A project whose repository has a GitHub remote and no [tracker]: `mason doctor` notes that its
#     issues are not imported (a note, not a failure); `mason task refresh` says so, with the config path.
#  2. A project with a tracker and a [self] limit (max_open = 4): the owner's issues are imported, an
#     outside issue is ignored until it carries the label; doctor shows the last poll and the counts.
#  3. Owner issues over the limit stay unimported; the summary counts each once, however often it looks,
#     and an alert names them. The monitors' landing view carries the same in one line per project.
#  4. A 404 from the tracker stops the poll: doctor, the alerts and the landing view show it with the
#     reason; once GitHub answers again, a refresh resumes it.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s61.XXXXXX); daemon=""; gh=""
cleanup() {
  [ -n "$daemon" ] && kill -9 "$daemon" 2>/dev/null || true
  [ -n "$gh" ] && kill -9 "$gh" 2>/dev/null || true
  rm -rf "$root"
}
trap cleanup EXIT
cargo build -q --workspace --bins
bin=target/debug
fx=scripts/exit/fixtures/s61
owner=${USER:-owner}
m() { "$bin/mason" --data-dir "$root/d" --config-dir "$root/c" "$@"; }
fail() { echo "FAILED: $*" >&2; m task list >&2 2>&1 || true; m alerts >&2 2>&1 || true; tail -20 "$root/masond.log" >&2 || true; exit 1; }
gh_set() { sed "s/OWNER/$owner/g" "$fx/issues-$1.json" > "$root/gh/issues.json.tmp"; mv "$root/gh/issues.json.tmp" "$root/gh/issues.json"; }
tasks() { m task list --json | grep -c '"id"' || true; }
alerts_have() { m alerts | grep -q "$1" || { m alerts >&2; fail "no alert saying: $1"; }; }
doctor_has() {  # PATTERN: some doctor line matches
  m doctor >"$root/doctor.out" 2>&1 || true
  grep -q "$1" "$root/doctor.out" || { cat "$root/doctor.out" >&2; fail "doctor does not say: $1"; }
}
landing() { python3 -I "$fx/landing.py" "$port"; }

echo "== setup: a fake GitHub, a project 'plain' (GitHub remote, no tracker) and a project 'mason' (tracker, [self] limit)"
mkdir -p "$root/gh" "$root/c/projects"
echo '[]' > "$root/gh/issues.json"   # nothing to import until claims are paused
"$bin/fake-github" "$root/gh" "$root/gh.port" 2>"$root/gh.log" &
gh=$!
until [ -s "$root/gh.port" ]; do sleep 0.05; done
ghport=$(cat "$root/gh.port")
git init -q -b main "$root/plain"
git -C "$root/plain" -c user.name=t -c user.email=t@t commit -q --allow-empty -m init
git -C "$root/plain" remote add origin git@github.com:acme/plain.git
git init -q --bare -b main "$root/origin.git"
git clone -q "$root/origin.git" "$root/repo" 2>/dev/null
git -C "$root/repo" checkout -q -b main
echo base > "$root/repo/base.txt"
git -C "$root/repo" add -A
git -C "$root/repo" -c user.name=t -c user.email=t@t commit -qm base
git -C "$root/repo" push -q origin main
cat > "$root/c/projects/plain.toml" <<TOML
path = "$root/plain"
base = "main"
test = "true"
TOML
cat > "$root/c/projects/mason.toml" <<TOML
path = "$root/repo"
base = "main"
remote = "origin"
test = "true"

[self]
max_open = 4
per_week = 10

[tracker]
kind = "github"
api = "http://127.0.0.1:$ghport"
repo = "o/r"
token = "env:FAKE_GITHUB_TOKEN"
interval_s = 300
TOML
export FAKE_GITHUB_TOKEN=recorded-test-token
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1])')
"$bin/masond" --data-dir "$root/d" --config-dir "$root/c" --port "$port" --heartbeat-ms 100 --sweep-ms 3600000 --owner "$owner" 2>>"$root/masond.log" &
daemon=$!
until m status >/dev/null 2>&1; do kill -0 "$daemon" || { cat "$root/masond.log" >&2; exit 1; }; sleep 0.1; done
m pause --mode claims --for 1h >/dev/null
# One task of the project's own, so there is a task to ask the tracker through (it takes one slot).
out=$(m task add "seed" --project mason --explored 2>&1) || fail "filing the seed: $out"
seed=$(echo "$out" | sed -n 's/^Filed task #\([0-9]*\).*/\1/p')

echo "== 1. no [tracker] section on a project with a GitHub remote"
m doctor >"$root/doctor.out" 2>&1 || true
grep '^note' "$root/doctor.out" | sed 's/^/   /'
grep -q '^note  project plain: issues on acme/plain are not imported; see docs/self-hosting.md$' "$root/doctor.out" || { cat "$root/doctor.out" >&2; fail "doctor does not note that plain's issues are not imported"; }
! grep -q 'issues on o/r' "$root/doctor.out" || fail "doctor noted a project that has a tracker"
! grep '^FAIL' "$root/doctor.out" | grep -q 'plain.*issues' || fail "the note counted as a failure"
out=$(m task add "something for plain" --project plain --explored 2>&1) || fail "filing in plain: $out"
pt=$(echo "$out" | sed -n 's/^Filed task #\([0-9]*\).*/\1/p')
m task refresh "$pt" | tee "$root/refresh.out" | sed 's/^/   /'
grep -q 'no tracker' "$root/refresh.out" || fail "refresh does not say there is no tracker"
grep -q "nothing is imported from its issues until a \[tracker\] section is added to $root/c/projects/plain.toml" "$root/refresh.out" || fail "refresh does not say what to do, with the path"
landing | tee "$root/landing.out" | sed 's/^/   /'
grep -q "plain: tracker off; issues are not imported, add \[tracker\] to $root/c/projects/plain.toml" "$root/landing.out" || fail "the landing view has no 'off' line for plain"

echo "== 2. an owner issue is imported; an outside issue is ignored until it is labeled"
gh_set 1
r=$(m task refresh "$seed"); echo "   first look: $r"
echo "$r" | grep -q '2 imported, 0 updated, 0 closed, 1 ignored, 0 unchanged, 0 waiting for a slot' || fail "the first look was not 2 imported, 1 ignored: $r"
[ "$(tasks)" = 4 ] || fail "expected plain's task, the seed and two imports"
doctor_has 'project mason: tracker o/r last polled .*: 2 imported, 1 ignored, 0 unchanged, 0 waiting for a slot'
grep 'tracker o/r' "$root/doctor.out" | sed 's/^/   /'
! m task list | grep -q "stranger's idea" || fail "an unlabeled outside issue was imported"
echo "   issues 201 and 202 (the owner's) are tasks; 204 (a stranger's, no label) is ignored"
gh_set 2
r=$(m task refresh "$seed")
echo "   after labeling 204: $r"
echo "$r" | grep -q '1 imported' || fail "the labeled issue was not imported: $r"
m task list | grep -q "stranger's idea" || fail "issue 204 is not a task"
echo "   issue 204 carries the label: imported"

echo "== 3. owner issues over max_open stay unimported; the count is per issue"
gh_set 3
r=$(m task refresh "$seed"); echo "   $r"
echo "$r" | grep -q '0 imported, 0 updated, 0 closed, 0 ignored, 3 unchanged, 2 waiting for a slot' || fail "the first look did not count 2 waiting: $r"
r=$(m task refresh "$seed"); echo "   $r"
echo "$r" | grep -q '3 unchanged, 2 waiting for a slot' || fail "a second look changed the count: $r"
[ "$(tasks)" = 5 ] || fail "an issue over the limit was imported ($(tasks) tasks)"
alerts_have '2 issue(s) wait for a slot (#205, #206)'
m alerts | grep 'wait for a slot' | sed 's/^/   /' | head -1
doctor_has 'project mason: tracker o/r last polled .*: 0 imported, 0 ignored, 3 unchanged, 2 waiting for a slot'
landing | tee "$root/landing.out" | sed 's/^/   /'
grep -q 'mason: tracker polling o/r: 0 imported, 0 ignored, 3 unchanged, 2 waiting for a slot' "$root/landing.out" || fail "the landing view lacks the tracker line"
echo "   still unimported, still waiting, counted once each"

echo "== 4. a 404 stops the poll, and it says why"
echo 404 > "$root/gh/listing-status"
r=$(m task refresh "$seed"); echo "   $r"
echo "$r" | grep -q 'stopped' || fail "refresh does not say the poll stopped: $r"
doctor_has 'WARN\|warn  project mason: the tracker poll for o/r stopped: .*404'
grep 'stopped' "$root/doctor.out" | sed 's/^/   /'
alerts_have 'the tracker poll stopped'
landing | tee "$root/landing.out" | sed 's/^/   /'
grep -q 'mason: tracker stopped: .*404' "$root/landing.out" || fail "the landing view does not show the stop and its reason"
rm "$root/gh/listing-status"
r=$(m task refresh "$seed"); echo "   after GitHub answers again: $r"
echo "$r" | grep -q 'asked the tracker: 0 imported, 0 updated, 0 closed, 0 ignored, 3 unchanged, 2 waiting for a slot' || fail "the poll did not resume: $r"
landing | grep -q 'mason: tracker polling' || fail "the landing view still says stopped"

echo "== nothing reached GitHub itself: every request went to the fake on loopback ($(wc -l < "$root/gh/requests.log" | tr -d ' ') requests)"
trap - EXIT; cleanup
echo "s61 OK"
