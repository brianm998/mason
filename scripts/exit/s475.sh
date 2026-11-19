#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s475 exit test: the daemon's integration tests build into a few executables (it plus the
# separate ones), and the number of tests is the number before the merge (931).
set -euo pipefail
cd "$(dirname "$0")/../.."
json=$(mktemp)
trap 'rm -f "$json"' EXIT
cargo test -q -p mason-daemon --no-run --message-format=json >"$json"
python3 -I - "$json" <<'PY'
import json, subprocess, sys
exes = []
for line in open(sys.argv[1]):
    try:
        m = json.loads(line)
    except ValueError:
        continue
    if (m.get("reason") == "compiler-artifact" and m.get("executable")
            and m.get("profile", {}).get("test") and "mason-daemon" in m["package_id"]):
        exes.append(m["executable"])
tests = 0
for e in exes:
    r = subprocess.run([e, "--list"], capture_output=True, text=True, cwd="crates/daemon", check=True)
    tests += sum(1 for x in r.stdout.splitlines() if x.endswith(": test"))
print(f"test executables: {len(exes)}, tests: {tests}")
assert len(exes) <= 24, "the daemon's tests are not merged"
# 931 is the macOS count. Four daemon tests are cfg(target_os = "macos"):
# taskcache.rs the_real_clone_keeps_mtimes; jobs.rs
# with_pty_the_command_runs_under_script_with_its_own_exit_code and
# a_stopped_pty_run_leaves_no_command_behind; update.rs
# installed_binaries_are_adhoc_signed_with_a_mason_identifier.
mac_only = ["the_real_clone_keeps_mtimes",
            "with_pty_the_command_runs_under_script_with_its_own_exit_code",
            "a_stopped_pty_run_leaves_no_command_behind",
            "installed_binaries_are_adhoc_signed_with_a_mason_identifier"]
names = ""
for e in exes:
    names += subprocess.run([e, "--list"], capture_output=True, text=True, cwd="crates/daemon", check=True).stdout
if sys.platform == "darwin":
    expected = 931
    assert all(n in names for n in mac_only), "macOS-only tests are missing"
else:
    expected = 931 - len(mac_only)
    assert not any(n in names for n in mac_only), "macOS-only tests listed on this platform"
assert tests >= expected, f"tests were dropped ({expected} expected on this platform)"
PY
echo "s475 OK"
