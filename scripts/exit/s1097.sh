#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s1097 exit test: scripts/test-parallel.py never gives cargo `--doc` beside a target selector or
# --no-run. Cargo and the test executables are faked; no build is started.
set -euo pipefail
cd "$(dirname "$0")/../.."
python3 -I - <<'PY'
import contextlib, importlib.util, io, subprocess, sys
spec = importlib.util.spec_from_file_location("tp", "scripts/test-parallel.py")
tp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(tp)

item = {"name": "x", "src": "tests/x.rs", "exe": "/nonexistent/x", "dir": ".", "pkg": "pkg"}

listed, ran = [], []

def run(command, items=(item,), exe_rc=0, doc_rc=0):
    """(status, listing calls, doc commands, output) of main() on a faked cargo."""
    del listed[:], ran[:]
    def list_executables(flags, root):
        listed.append(flags)
        return list(items)
    def doc_run(cmd, **kw):
        ran.append(cmd)
        return subprocess.CompletedProcess(cmd, doc_rc, stdout=b"")
    tp.list_executables = list_executables
    tp.run_one = lambda i, filters, libtest: (exe_rc, "", 0.0)
    tp.subprocess.run = doc_run
    sys.argv = ["test-parallel.py", "--"] + command.split()
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        rc = tp.main()
    return rc, listed, ran, out.getvalue()

# A target selector: no doc run, and the summary does not claim one.
for sel in ["--lib", "--bins", "--bin b", "--bin=b", "--test x", "--test=x", "--tests", "--benches",
            "--bench b", "--bench=b", "--examples", "--example e", "--example=e", "--all-targets"]:
    rc, listed, ran, out = run("cargo test -p pkg " + sel)
    assert rc == 0 and not ran, (sel, rc, ran)
    assert all("--doc" not in f for f in listed), (sel, listed)
    assert "1 executables" in out and "doc tests" not in out, (sel, out)
rc, _, ran, _ = run("cargo test --test x", exe_rc=101)
assert rc == 1 and not ran, (rc, ran)

# --doc: only the doc tests, nothing listed, never --no-run.
rc, listed, ran, out = run("cargo test --workspace --doc --no-run name")
assert rc == 0 and not listed, (rc, listed)
assert ran == [["cargo", "test", "--workspace", "--doc", "name"]], ran
assert "doc tests only" in out, out
rc, _, _, out = run("cargo test --doc", doc_rc=101)
assert rc == 1 and "1 failed" in out, (rc, out)

# No selector: the doc command as before.
rc, listed, ran, out = run("cargo test --workspace name -- --nocapture")
assert rc == 0 and len(listed) == 1, (rc, listed)
assert ran == [["cargo", "test", "--workspace", "--doc", "name", "--", "--nocapture"]], ran
assert "and doc tests" in out, out
assert run("cargo test --workspace", doc_rc=101)[0] == 1

# Nothing to run outside a doc run is still a failure.
rc, _, ran, out = run("cargo test --lib", items=())
assert rc == 1 and not ran and "no test executable found" in out, (rc, ran, out)

# Both at once: refused by name, before cargo runs.
for command in ["cargo test --doc --lib", "cargo test --test=x --doc"]:
    try:
        run(command)
    except SystemExit as e:
        assert "--doc cannot be combined with a target selector" in str(e.code), e.code
    else:
        raise AssertionError("not refused: " + command)
    assert not listed and not ran, (command, listed, ran)
PY
echo "s1097 OK"
