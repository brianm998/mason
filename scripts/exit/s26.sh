#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 26 exit test: installer and onboarding, with the fake agent. No network, no real LLM,
# no real release, no GitHub.
#  1. The five templates exist, are MIT, and each mason.toml passes `mason definition check`.
#  2. scripts/install.sh against a local fake release: a good checksum installs, a bad one
#     installs nothing and says so, a clashing `mason` (Dart's) is named with how to resolve
#     it, and signing is reported as not verified.
#  3. Sample Rust, Python and Swift repositories, one passing test each. For each template on
#     each repository: `mason init --workflow T --yes` writes the setup branch (suites run once,
#     baseline recorded, Filing notes, the starting-point sentence), the branch is merged and
#     pushed to a local bare origin as the owner would, `mason doctor` is clean of failures, and
#     `mason init --prove-only` files the harmless task, which then lands with fake-claude.
#     (Pull-request landing needs a git host, so `--landing direct` is given; the PR modes are
#     checked by the unit tests that parse their landing.toml and by s26-real.sh, not here.)
#  4. `mason remove --yes` takes it out again and leaves the tracker's tasks.
# A real install timed against the 10-minute target is s26-manual.sh (not run here).
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(mktemp -d /tmp/mason-s26.XXXXXX); daemon=""
stop_daemon() {
  if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi
  daemon=""
  for f in "$root"/*/d/h/*/host.kv; do
    [ -e "$f" ] || continue
    hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f")
    if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi
  done
}
cleanup() { stop_daemon; rm -rf "$root"; }
trap cleanup EXIT

cargo build -q --workspace --bins
bin=$PWD/target/debug
fail() { echo "FAIL: $*" >&2; [ -f "${LOG:-/nonexistent}" ] && tail -20 "$LOG" >&2; exit 1; }
git_id=(-c user.name=t -c user.email=t@t)

echo "== 1. templates"
templates="solo-fast reviewed-pr batched-ci device-mobile human-gated"
for t in $templates; do
  for f in mason.toml landing.toml; do
    [ -f "templates/$t/$f" ] || fail "templates/$t/$f is missing"
    head -n 1 "templates/$t/$f" | grep -qx '# SPDX-License-Identifier: MIT' || fail "templates/$t/$f is not MIT"
  done
  "$bin/mason" definition check "templates/$t/mason.toml" > "$root/check.txt" 2>&1 || { cat "$root/check.txt" >&2; fail "templates/$t/mason.toml does not pass definition check"; }
  echo "  $t: $(head -n 1 "$root/check.txt")"
done
echo "PASS: five templates, each a valid definition"

echo "== 2. the install script, against a local fake release"
rel="$root/release"; mkdir -p "$rel/pkg"
for b in mason masond step-host mason-mcp; do
  printf '#!/bin/sh\necho "mason: command-line client for the MASON pipeline (fake %s)"\n' "$b" > "$rel/pkg/$b"; chmod +x "$rel/pkg/$b"
done
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) tgt=aarch64-apple-darwin ;; Darwin-x86_64) tgt=x86_64-apple-darwin ;;
  Linux-aarch64|Linux-arm64) tgt=aarch64-unknown-linux-gnu ;; Linux-x86_64) tgt=x86_64-unknown-linux-gnu ;;
  *) fail "no target for this machine" ;;
esac
tar -czf "$rel/mason-9.9.9-$tgt.tar.gz" -C "$rel/pkg" .
sha=$( (sha256sum "$rel/mason-9.9.9-$tgt.tar.gz" 2>/dev/null || shasum -a 256 "$rel/mason-9.9.9-$tgt.tar.gz") | cut -d' ' -f1)
echo "$sha  mason-9.9.9-$tgt.tar.gz" > "$rel/SHA256SUMS"
inst() { bash scripts/install.sh --version 9.9.9 --base-url "file://$rel" --prefix "$1"; }
mkdir -p "$root/clash"; printf '#!/bin/sh\necho "Bricks, templates for Dart: mason_cli 0.1"\n' > "$root/clash/mason"; chmod +x "$root/clash/mason"
# good checksum, clashing mason first on PATH
out=$(PATH="$root/clash:$PATH" inst "$root/p1" 2>&1) || { echo "$out" >&2; fail "install with a good checksum failed"; }
echo "$out" | sed 's/^/  /'
for b in mason masond step-host mason-mcp; do [ -x "$root/p1/bin/$b" ] || fail "$b was not installed"; done
echo "$out" | grep -q 'checksum ok' || fail "no checksum confirmation"
echo "$out" | grep -q 'signature NOT verified' || fail "signing was not reported as unverified"
echo "$out" | grep -q 'different `mason` is first on your PATH' || fail "the clashing mason was not reported"
echo "$out" | grep -q 'Bricks, templates for Dart' || fail "the clash did not say what the other mason is"
echo "$out" | grep -q 'dart pub global deactivate mason_cli' || fail "the clash did not say how to resolve it"
# no clash: the installed one is first
out=$(PATH="$root/p1/bin:$PATH" inst "$root/p1" 2>&1) || fail "reinstall failed"
echo "$out" | grep -q 'the one just installed' || fail "the installed mason was not recognised: $out"
# a tampered archive installs nothing
cp "$rel/mason-9.9.9-$tgt.tar.gz" "$root/orig.tgz"; echo junk >> "$rel/mason-9.9.9-$tgt.tar.gz"
if out=$(inst "$root/p2" 2>&1); then fail "a tampered archive was installed"; fi
echo "$out" | grep -q 'CHECKSUM MISMATCH' || fail "no mismatch message: $out"
[ ! -e "$root/p2/bin/mason" ] || fail "something was installed after a checksum failure"
cp "$root/orig.tgz" "$rel/mason-9.9.9-$tgt.tar.gz"
# a release with no checksum file is refused
mv "$rel/SHA256SUMS" "$rel/SHA256SUMS.off"
if inst "$root/p3" >/dev/null 2>&1; then fail "installed with no SHA256SUMS"; fi
mv "$rel/SHA256SUMS.off" "$rel/SHA256SUMS"
echo "PASS: checksum verified, tampering and a missing checksum refused, clash named with its fix, signing said to be unverified"

echo "== 3. init for each template on Rust, Python and Swift samples, then a landed task"
samples="$root/samples"; mkdir -p "$samples"
mk_sample() {  # LANG
  local d="$samples/$1"; mkdir -p "$d"
  case "$1" in
    rust)
      mkdir -p "$d/src"
      printf '[package]\nname = "sample"\nversion = "0.1.0"\nedition = "2021"\n' > "$d/Cargo.toml"
      printf 'pub fn add(a: i32, b: i32) -> i32 { a + b }\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn adds() { assert_eq!(super::add(1, 2), 3); }\n}\n' > "$d/src/lib.rs"
      printf 'target/\nCargo.lock\n' > "$d/.gitignore" ;;
    python)
      printf 'def add(a, b):\n    return a + b\n' > "$d/calc.py"
      printf 'import unittest\nimport calc\n\nclass CalcTests(unittest.TestCase):\n    def test_add(self):\n        self.assertEqual(calc.add(1, 2), 3)\n\nif __name__ == "__main__":\n    unittest.main()\n' > "$d/test_calc.py"
      printf '' > "$d/requirements.txt"; printf '__pycache__/\n' > "$d/.gitignore" ;;
    swift)
      mkdir -p "$d/Sources/Sample" "$d/Tests/SampleTests"
      printf '// swift-tools-version:5.5\nimport PackageDescription\nlet package = Package(\n    name: "Sample",\n    targets: [\n        .target(name: "Sample"),\n        .testTarget(name: "SampleTests", dependencies: ["Sample"]),\n    ]\n)\n' > "$d/Package.swift"
      printf 'public func plus(_ a: Int, _ b: Int) -> Int { a + b }\n' > "$d/Sources/Sample/Sample.swift"
      printf 'import XCTest\n@testable import Sample\n\nfinal class SampleTests: XCTestCase {\n    func testAdd() { XCTAssertEqual(plus(1, 2), 3) }\n}\n' > "$d/Tests/SampleTests/SampleTests.swift"
      printf '.build/\n' > "$d/.gitignore" ;;
  esac
}
have() { command -v "$1" >/dev/null 2>&1; }
langs=""
for l in rust python swift; do
  case "$l" in rust) tool=cargo ;; python) tool=python3 ;; swift) tool=swift ;; esac
  if have "$tool"; then langs="$langs $l"; else echo "SKIPPED: $l samples, no $tool on this machine" >&2; fi
done
[ -n "$langs" ] || fail "no toolchain at all"

for l in $langs; do mk_sample "$l"; done

# One template on one sample: init, merge, daemon, prove, land, (remove). Runs in a subshell of
# its own with its own daemon and directories, so the five templates of a language go at once
# (a cold `swift test` is 40 s and would otherwise make this script ten minutes long).
pair() {  # N LANG TEMPLATE
  n=$1; lang=$2; t=$3; p="$root/q$1"; daemon=""
  LOG="$p/masond.log"
  trap 'if [ -n "$daemon" ]; then kill -9 "$daemon" 2>/dev/null || true; wait "$daemon" 2>/dev/null || true; fi; for f in "$p"/d/h/*/host.kv; do [ -e "$f" ] || continue; hp=$(sed -n "s/^host_pid=//p" "$f"); cp=$(sed -n "s/^child_pid=//p" "$f"); if [ -n "$hp" ] && ps -o command= -p "$hp" 2>/dev/null | grep -qF "step-host --dir $(dirname "$f")"; then kill -9 "-$cp" 2>/dev/null || true; kill -9 "$hp" 2>/dev/null || true; fi; done' EXIT
  mkdir -p "$p"
  cp -R "$samples/$lang" "$p/repo"
  git init -q --bare -b main "$p/origin.git"
  git -C "$p/repo" init -q -b main
  git -C "$p/repo" add -A; git -C "$p/repo" "${git_id[@]}" commit -qm init
  git -C "$p/repo" remote add origin "$p/origin.git"; git -C "$p/repo" push -q origin main
  m() { (cd "$p/repo" && "$bin/mason" --data-dir "$p/d" --config-dir "$p/c" "$@"); }
  # The agent's script: change the repository, submit; approve any review.
  cat > "$p/agent.sh" <<SH
printf 'MASON landed this task.\n' > MASON-WORKS.md
git add -A; git -c user.name=t -c user.email=t@t commit -qm "prove MASON works"
SH
  turn='[{"sh": "sh '"$p"'/agent.sh"}, {"call": {"tool": "submit_change", "args": {"summary": "added the file", "head": "$HEAD"}}}]'
  rev='[{"call": {"tool": "submit_review", "args": {"verdict": "approve", "summary": "fine"}}}]'
  echo '{"steps": {"develop": ['"$turn,$turn,$turn"'], "review": ['"$rev,$rev,$rev"']}}' > "$p/script.json"

  out=$(m init --workflow "$t" --yes --no-prove --landing direct --agent-bin "$bin/fake-claude" --id sample 2>&1) || { echo "$out" >&2; fail "init failed for $t on $lang"; }
  echo "$out" | grep -qF 'This is only a starting point. You can change anything, any time, in the monitor or in `.mason/`.' || fail "the starting-point sentence is missing ($t, $lang)"
  echo "$out" | grep -q "$lang: 1 test(s) found, 1 passed, 0 failed" || { echo "$out" >&2; fail "the $lang suite was not run once and parsed ($t)"; }
  br=$(git -C "$p/repo" show mason/setup:.mason/baselines.toml) || fail "no baseline on the setup branch"
  echo "$br" | grep -q "executed = 1" && echo "$br" | grep -q "green = true" || fail "baseline not recorded: $br"
  echo "$br" | grep -q '^seconds = ' || fail "typical time not recorded"
  git -C "$p/repo" show mason/setup:AGENTS.md | grep -q 'explored: true' || fail "no Filing notes on the setup branch"
  [ ! -e "$p/repo/.mason" ] || fail "init touched the working tree"
  # The owner reviews and merges the branch, pushes it, and goes back to their own work.
  git -C "$p/repo" merge -q --ff-only mason/setup
  git -C "$p/repo" push -q origin main
  git -C "$p/repo" checkout -q -b owner-work
  printf '\n[agent_env]\nFAKE_CLAUDE_SCRIPT = "%s/script.json"\n' "$p" >> "$p/c/projects/sample.toml"
  if m doctor 2>&1 | grep -q '^FAIL' ; then m doctor | grep '^FAIL' >&2; fail "doctor reports failures for $t on $lang"; fi

  "$bin/masond" --port 0 --data-dir "$p/d" --config-dir "$p/c" --heartbeat-ms 100 --sweep-ms 3600000 --max-concurrent 2 2>>"$LOG" &
  daemon=$!
  until m status >/dev/null 2>&1; do kill -0 "$daemon" 2>/dev/null || fail "masond did not start"; sleep 0.1; done
  out=$(m init --prove-only --id sample 2>&1) || { echo "$out" >&2; fail "the proof task was not filed ($t, $lang)"; }
  id=$(echo "$out" | sed -n 's/.*Filed task #\([0-9]*\).*/\1/p')
  [ -n "$id" ] || fail "no task id in: $out"
  end=$((SECONDS + 240)); state=""
  until [ "$state" = landed ]; do
    state=$(m task show "$id" --json 2>/dev/null | sed -n 's/^  "state": "\(.*\)",$/\1/p')
    [ "$state" != failed ] || { m task show "$id" >&2; fail "the task failed ($t, $lang)"; }
    [ $SECONDS -lt $end ] || { m task show "$id" >&2; fail "timed out; task is $state ($t, $lang)"; }
    # The human-gated workflow waits for the owner twice: approve, as the owner would.
    [ "$t" != human-gated ] || m task approve "$id" >/dev/null 2>&1 || true
    sleep 0.5
  done
  git -C "$p/origin.git" show main:MASON-WORKS.md | grep -qx 'MASON landed this task.' || fail "the landed change is not on origin/main ($t, $lang)"
  echo "  $t on $lang: init, merge, task #$id landed"

  # 4. Clean removal, once per language on the first template.
  if [ "$t" = solo-fast ]; then
    git -C "$p/repo" checkout -q main
    m remove > "$p/rm.txt" 2>&1 && fail "remove without --yes changed things"
    grep -q 'Would remove' "$p/rm.txt" || fail "remove did not list"
    m remove --yes > "$p/rm.txt" 2>&1 || { cat "$p/rm.txt" >&2; fail "remove --yes failed"; }
    [ ! -e "$p/repo/.mason" ] && [ ! -e "$p/c/projects/sample.toml" ] || fail "remove left .mason/ or the project file"
    [ ! -e "$p/repo/AGENTS.md" ] || fail "remove left the Filing notes"
    grep -q 'tracker tasks' "$p/rm.txt" || fail "remove did not say it leaves tracker tasks"
    [ -f "$p/d/daemon.json" ] || true
  fi
}

n=0; pids=(); outs=()
for lang in $langs; do
  for t in $templates; do
    n=$((n + 1)); mkdir -p "$root/q$n"
    ( pair "$n" "$lang" "$t" ) > "$root/q$n/out.txt" 2>&1 &
    pids+=($!); outs+=("$root/q$n/out.txt")
  done
  for i in "${!pids[@]}"; do
    wait "${pids[$i]}" || { cat "${outs[$i]}" >&2; fail "a template/sample pair failed (see above)"; }
    cat "${outs[$i]}"
  done
  pids=(); outs=()
done
echo "PASS: $n template/sample pairs: init wrote the branch, suites ran once with baselines, a harmless task landed with fake-claude on each"
echo "s26: all checks passed"
