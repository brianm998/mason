#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Stage 27 exit test for a star `.mason/` folder: the draft in docs/migrations/star/.mason by default, or
# the folder given as the first argument (star's own, once it has landed). No network, no star build,
# no LLM. It fails if any of these is false:
#   1. the folder validates (`mason definition check`);
#   2. every suite sets the CPU merge explicitly (STAR_USE_GPU_FOR_MERGE=0): the GPU merge crashes the Mac;
#   3. no suite runs `starUITests` (it wakes the display and drives the mouse), none wraps a command in
#      `caffeinate`, and an `xcodebuild test` names only `-only-testing:starTests`;
#   4. every suite that runs a heavy command does so through `pipeline/bin/starpipe heavy --` and carries
#      the `# s27-shadow: remove at the switch` comment once. With S27_AFTER_SWITCH=1 (after the switch)
#      the opposite holds: no wrapper and no comment anywhere;
#   5. a dry run of one task through the graph with the fake agent (no LLM) ends Landed.
# Run against the draft it also checks the project registration (docs/examples/star.project.toml), the
# extension scripts (the shard parser on recorded xctest logs, the artifact links in scratch git
# repositories), and that the checks above do fail on copies with a starUITests suite and without the key.
# MASON_BIN names a built `mason` to use instead of building one.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)"
root=$(pwd)
draft=docs/migrations/star/.mason
dir=${1:-$draft}
fx=scripts/exit/fixtures/s27
fail() { echo "s27: FAILED: $*" >&2; exit 1; }
[ -f "$dir/mason.toml" ] || fail "$dir/mason.toml does not exist"
[ -f "$dir/landing.toml" ] || fail "$dir/landing.toml does not exist"

# The static checks, over one folder. Prints what is wrong; status 1 if anything is.
lint() {
  python3 -I - "$1" "${S27_AFTER_SWITCH:-0}" <<'PY'
import re, sys, tomllib
from pathlib import Path

folder, after = Path(sys.argv[1]), sys.argv[2] == "1"
raw = (folder / "mason.toml").read_text()
doc = tomllib.loads(raw)
KEY = "STAR_USE_GPU_FOR_MERGE=0"
WRAP = "starpipe heavy --"
MARK = "# s27-shadow: remove at the switch"
HEAVY = re.compile(r"\bswift\s+(build|test|run)\b|\bxcodebuild\b|\bgradlew?\b|star-gherkin|shards\.py|gherkin/generate\.sh|\.build/[\w-]+/(star|stard)\b")
bad = []

# each suite's own text: its table header to the next header, so its comments belong to it
blocks, cur = {}, None
for line in raw.splitlines():
    m = re.match(r"^\[suite\.([^\]]+)\]\s*$", line)
    if m:
        cur = m.group(1).strip('"')
        blocks[cur] = []
    elif re.match(r"^\[", line):
        cur = None
    elif cur is not None:
        blocks[cur].append(line)

suites = doc.get("suite", {})
if not suites:
    bad.append("the definition has no suites")
for name, s in suites.items():
    cmd = s.get("command", "")
    if KEY not in cmd:
        bad.append(f"suite {name}: does not set {KEY} (the GPU merge must never be left to the default)")
    if "starUITests" in cmd:
        bad.append(f"suite {name}: runs starUITests, which wakes the display and drives the mouse; those stay on CI")
    if "caffeinate" in cmd:
        bad.append(f"suite {name}: wraps a command in caffeinate; `-u` turns a sleeping display on")
    if re.search(r"\bxcodebuild\b", cmd) and re.search(r"(^|\s)(test|test-without-building)(\s|$)", cmd):
        only = re.findall(r"-only-testing:(\S+)", cmd)
        if not only or any(not o.startswith("starTests") for o in only):
            bad.append(f"suite {name}: an xcodebuild test must name only -only-testing:starTests")
    text = "\n".join(blocks.get(name, []))
    marks = text.count(MARK)
    wraps = cmd.count(WRAP)
    if after:
        if wraps or marks:
            bad.append(f"suite {name}: still has the shadow wrapper or its comment after the switch")
    else:
        if HEAVY.search(cmd) and wraps != 1:
            bad.append(f"suite {name}: runs a heavy command without `pipeline/bin/starpipe heavy --` (exactly once)")
        if wraps and marks != 1:
            bad.append(f"suite {name}: has the wrapper but {marks} `{MARK}` comments (needs exactly one)")
        if marks and not wraps:
            bad.append(f"suite {name}: has the switch comment but no wrapper")
for n in blocks:
    if n not in suites:
        bad.append(f"suite {n}: could not be read")
land = tomllib.loads((folder / "landing.toml").read_text())
if land.get("mode") != "direct-ci":
    bad.append(f"landing.toml: mode is {land.get('mode')!r}, the proposed mode is direct-ci")
print("\n".join(bad))
sys.exit(1 if bad else 0)
PY
}

echo "== the static checks on $dir"
out=$(lint "$dir") || { echo "$out" >&2; fail "static checks on $dir"; }
echo "ok: every suite sets the CPU merge; no starUITests, no caffeinate; shadow wrappers as expected; direct-ci"

echo "== mason definition check"
if [ -n "${MASON_BIN:-}" ]; then mason=$MASON_BIN; else
  cargo build -q --workspace --bins || fail "cargo build"
  mason=target/debug/mason
fi
[ -x "$mason" ] || fail "no mason binary at $mason"
"$mason" definition check "$dir/mason.toml" >"${TMPDIR:-/tmp}/s27-check.$$" || { cat "${TMPDIR:-/tmp}/s27-check.$$" >&2; rm -f "${TMPDIR:-/tmp}/s27-check.$$"; fail "$dir/mason.toml does not validate"; }
head -3 "${TMPDIR:-/tmp}/s27-check.$$"; rm -f "${TMPDIR:-/tmp}/s27-check.$$"

echo "== a dry run with the fake agent, no LLM"
dry=$("$mason" definition dry-run "$dir/mason.toml" --scenarios "$fx/scenarios.toml") || fail "dry run"
echo "$dry"
first=$(echo "$dry" | sed -n '2p')
case "$first" in *"=>  landed") ;; *) fail "the first dry run did not end Landed: $first" ;; esac
[ "$(echo "$dry" | grep -c '=>  landed')" -eq "$(echo "$dry" | grep -c 'started by')" ] || fail "a dry run did not end Landed"

if [ "$dir" = "$draft" ]; then
  echo "== the project registration"
  python3 -I - docs/examples/star.project.toml <<'PY' || fail "docs/examples/star.project.toml"
import sys, tomllib
d = tomllib.load(open(sys.argv[1], "rb"))
t, c = d.get("tracker", {}), d.get("ci", {})
want = [
    (d.get("base") == "develop", "base is develop"),
    (t.get("kind") == "github" and t.get("repo") == "brianm998/starry_timelapse_airplane_remover", "tracker repo"),
    (t.get("label") == "mason", "label mason"),
    (t.get("import_as") == "proposed", "import_as proposed"),
    ("self" not in d, "no [self] section"),
    (c.get("workflow") == "pr-gate.yml" and c.get("kind") == "github-actions", "[ci] pr-gate.yml"),
    (all(str(x.get("token", "")).startswith(("env:", "file:")) for x in (t, c)), "tokens are references"),
]
bad = [m for ok, m in want if not ok]
if bad:
    print("registration is wrong: " + ", ".join(bad), file=sys.stderr)
    sys.exit(1)
print("ok: base develop, label mason, import_as proposed, no [self], tokens are references")
PY

  echo "== the shard parser, on recorded xctest logs"
  tmp=$(mktemp -d /tmp/mason-s27.XXXXXX)
  trap 'rm -rf "$tmp"' EXIT
  python3 -I "$dir/scripts/shards.py" --parse-only "$fx/shard-ok.log" --junit "$tmp/ok.xml" >/dev/null || fail "shards.py on a green log"
  grep -q 'tests="3" failures="0" skipped="1"' "$tmp/ok.xml" || fail "shards.py wrote the wrong JUnit for a green log"
  python3 -I "$dir/scripts/shards.py" --parse-only "$fx/shard-red.log" --junit "$tmp/red.xml" >/dev/null && fail "shards.py passed a red log"
  grep -q '<failure' "$tmp/red.xml" || fail "shards.py wrote no <failure> for a red log"
  echo "ok: a green log is green with its skip counted, a red one is red"

  echo "== the artifact links, in scratch repositories"
  links="opencv/include opencv/lib opencv/opencv2.framework StarDecisionTrees/include StarDecisionTrees/lib gui/videos external_binaries/bin"
  git init -q "$tmp/main" && git -C "$tmp/main" -c user.name=s27 -c user.email=s27@example.com commit -q --allow-empty -m init
  for p in $links; do mkdir -p "$tmp/main/$p"; done
  git -C "$tmp/main" worktree add -q "$tmp/wt" -b s27-scratch
  (cd "$tmp/wt" && bash "$root/$dir/scripts/link-artifacts.sh" >/dev/null) || fail "link-artifacts.sh failed"
  for p in $links; do [ -L "$tmp/wt/$p" ] || fail "link-artifacts.sh did not link $p"; done
  (cd "$tmp/wt" && bash "$root/$dir/scripts/link-artifacts.sh" >/dev/null) || fail "link-artifacts.sh is not repeatable"
  rm "$tmp/wt/gui/videos" && mkdir "$tmp/wt/gui/videos"
  (cd "$tmp/wt" && bash "$root/$dir/scripts/link-artifacts.sh" >/dev/null 2>&1) && fail "link-artifacts.sh replaced a real directory"
  echo "ok: seven links, repeatable, and it refuses to replace a real directory"

  echo "== the checks do fail on a bad copy"
  mkdir -p "$tmp/bad1" "$tmp/bad2"
  for b in bad1 bad2; do cp "$dir/mason.toml" "$dir/landing.toml" "$tmp/$b/"; done
  printf '\n[suite.test-ui]\n# s27-shadow: remove at the switch\ncommand = "pipeline/bin/starpipe heavy -- env STAR_USE_GPU_FOR_MERGE=0 xcodebuild -scheme star -only-testing:starUITests test"\nparser = "xctest"\nlevel = "t1"\n' >> "$tmp/bad1/mason.toml"
  lint "$tmp/bad1" >/dev/null 2>&1 && fail "the checks passed a copy with starUITests added"
  sed 's/STAR_USE_GPU_FOR_MERGE=0 swift test --package-path logging/swift test --package-path logging/' "$dir/mason.toml" >"$tmp/bad2/mason.toml"
  cmp -s "$dir/mason.toml" "$tmp/bad2/mason.toml" && fail "the self-test could not remove the CPU-merge key"
  lint "$tmp/bad2" >/dev/null 2>&1 && fail "the checks passed a copy without the CPU-merge key"
  echo "ok: a starUITests suite and a missing CPU-merge key are both refused"
fi
echo "s27: all checks passed"
