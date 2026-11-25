# SPDX-License-Identifier: GPL-3.0-or-later
"""Run one shard of a cargo workspace's tests (#701, decision 0722).

    python3 -I scripts/test-shard.py [--run-json F] [--shard K] [--shards N] -- <cargo test command>

MASON splits a suite across several GitHub runners. The daemon writes each shard's tests into
`.mason/run.json` (`shards`, `shard_lists`: one list of test names per shard, shard 1 first) and each
runner sets MASON_SHARD (1-based). This builds the tests with `<cargo test ...> --no-run`, lists
each test executable's tests (`<exe> --list`), keeps the ones that belong to this shard and runs
them with `--exact`. libtest's lines are printed unchanged, under cargo's own
`     Running <src> (<exe>)` line, so the suite's regex parser reads them as it does for one run.

A test belongs to the shard whose list names it. A test in no list (new since the times were
recorded) belongs to shard `fnv1a64(name) % shards + 1`, so every test runs on exactly one shard.
An executable that cannot list its tests (`harness = false`) runs whole, on the shard its target
name hashes to. Doc tests are listed and split the same way, with `cargo test --doc`; a command
that selects targets (`--test x`, `--lib`) runs none, and `--doc` runs only them.

The exit status is 0 only when every executable run passed. Leading NAME=value words of the cargo
command are set in the environment of everything this runs. Each executable runs in its package's
directory with CARGO_MANIFEST_DIR set, as cargo does; RUSTC_BOOTSTRAP=1 is set for it when the
libtest arguments contain `unstable-options` (the suite's `--report-time`).
"""

import argparse
import json
import os
import re
import signal
import subprocess
import sys

# `cargo test` flags that take a value in the next argument.
VALUE_FLAGS = {
    "-p", "--package", "--exclude", "--target-dir", "--manifest-path", "--features", "-F",
    "--test", "--bin", "--example", "--bench", "-j", "--jobs", "--target", "--profile",
    "--message-format", "--color", "--config", "-Z", "--lockfile-path", "--timings",
}

# `cargo test` flags that pick targets. Cargo refuses `--doc` beside any of them.
TARGET_FLAGS = {
    "--lib", "--bins", "--bin", "--tests", "--test", "--benches", "--bench", "--examples",
    "--example", "--all-targets",
}

LISTED = re.compile(r"^(.*): (test|benchmark)$")


def fnv1a(name):
    h = 0xCBF29CE484222325
    for b in name.encode("utf-8"):
        h ^= b
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


def shard_of(lists, name, shards):
    """1-based shard of a test: the list that names it, else its hash's."""
    for i, names in enumerate(lists):
        if name in names:
            return i + 1
    return fnv1a(name) % shards + 1


def split_cargo(argv):
    """(env, cargo flags, name filters, libtest args) from the words after our own `--`."""
    env = {}
    while argv and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", argv[0]):
        k, v = argv[0].split("=", 1)
        env[k] = v
        argv = argv[1:]
    if "test" not in argv:
        raise SystemExit("test-shard: the command must be a `cargo test` command")
    at = argv.index("test")
    flags, filters, libtest = list(argv[: at + 1]), [], []
    i = at + 1
    while i < len(argv):
        a = argv[i]
        if a == "--":
            libtest = argv[i + 1:]
            break
        if a in VALUE_FLAGS and i + 1 < len(argv):
            flags += [a, argv[i + 1]]
            i += 2
            continue
        if a == "--no-run":
            pass
        elif a.startswith("-"):
            flags.append(a)
        else:
            filters.append(a)
        i += 1
    return env, flags, filters, libtest


def list_executables(flags, root, env):
    """Build the tests and return [{name, src, exe, dir, pkg}] from cargo's JSON messages."""
    cmd = flags + ["--no-run", "--message-format=json-render-diagnostics"]
    p = subprocess.Popen(cmd, cwd=root, env=env, stdout=subprocess.PIPE, stderr=sys.stderr, text=True)
    found, seen = [], set()
    for line in p.stdout:
        try:
            m = json.loads(line)
        except ValueError:
            sys.stdout.write(line)
            continue
        if m.get("reason") == "compiler-message":
            rendered = m.get("message", {}).get("rendered")
            if rendered:
                sys.stderr.write(rendered)
            continue
        if m.get("reason") != "compiler-artifact":
            continue
        if not m.get("profile", {}).get("test") or not m.get("executable"):
            continue
        exe = m["executable"]
        if exe in seen:
            continue
        seen.add(exe)
        target = m["target"]
        pkg_dir = os.path.dirname(m["manifest_path"])
        try:
            src = os.path.relpath(target["src_path"], pkg_dir)
        except ValueError:
            src = target["src_path"]
        if any(k in ("lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "bin")
               for k in target.get("kind", [])):
            src = "unittests " + src
        pkg = m.get("package_id", "").split("#")[-1].split("@")[0].split("/")[-1].split(" ")[0]
        found.append({"name": target["name"], "src": src, "exe": exe, "dir": pkg_dir, "pkg": pkg})
    rc = p.wait()
    if rc != 0:
        raise SystemExit(f"test-shard: `{' '.join(cmd)}` failed with status {rc}")
    return found


def exe_env(item, env, libtest):
    e = dict(env)
    e["CARGO_MANIFEST_DIR"] = item["dir"]
    if item["pkg"]:
        e["CARGO_PKG_NAME"] = item["pkg"]
    if any("unstable-options" in a for a in libtest):
        e["RUSTC_BOOTSTRAP"] = "1"
    return e


def run(cmd, cwd, env, limit=None):
    """(status, output). With `limit` seconds, a run that outlasts it is killed (with its
    process group) and ends `(124, output so far + a timeout line)`."""
    p = subprocess.Popen(cmd, cwd=cwd, env=env, stdin=subprocess.DEVNULL, start_new_session=bool(limit),
                         stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    try:
        out, _ = p.communicate(timeout=limit)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except OSError:
            p.kill()
        out, _ = p.communicate()
        text = out.decode("utf-8", "replace")
        return 124, text + ("" if text.endswith("\n") or not text else "\n") + f"(timed out after {limit:g} s)"
    return p.returncode, out.decode("utf-8", "replace")


def exe_limit():
    """MASON_TEST_EXE_LIMIT_S as seconds, or None (no deadline) when unset or not positive."""
    try:
        v = float(os.environ.get("MASON_TEST_EXE_LIMIT_S", ""))
    except ValueError:
        return None
    return v if v > 0 else None


def listing(out):
    """The tests in `--list` output, or None when this was not libtest's listing."""
    names = []
    for line in out.splitlines():
        m = LISTED.match(line)
        if m and m.group(2) == "test":
            names.append(m.group(1))
    if names or re.search(r"^\d+ tests?, \d+ benchmarks?$", out, re.M):
        return names
    return None


def emit(text):
    sys.stdout.write(text + ("" if text.endswith("\n") or not text else "\n"))
    sys.stdout.flush()


def main():
    ap = argparse.ArgumentParser(usage="test-shard.py [--run-json F] [--shard K] [--shards N] -- <cargo test command>")
    ap.add_argument("--run-json", default=".mason/run.json")
    ap.add_argument("--shard", type=int, default=int(os.environ.get("MASON_SHARD", "0") or "0"))
    ap.add_argument("--shards", type=int, default=0)
    ap.add_argument("cargo", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    words = args.cargo[1:] if args.cargo[:1] == ["--"] else args.cargo
    if not words:
        ap.error("give the cargo test command after --")
    try:
        with open(args.run_json, encoding="utf-8") as f:
            plan = json.load(f)
    except (OSError, ValueError) as e:
        raise SystemExit(f"test-shard: cannot read {args.run_json}: {e}")
    lists = plan.get("shard_lists") or []
    shards = args.shards or int(plan.get("shards") or 0) or len(lists) or 1
    shard = args.shard
    if not 1 <= shard <= shards:
        raise SystemExit(f"test-shard: shard {shard} is not one of 1..{shards} (set MASON_SHARD)")
    lists = [set(l) for l in lists]

    penv, flags, filters, libtest = split_cargo(words)
    env = dict(os.environ)
    env.update(penv)
    root = os.getcwd()
    # Cargo refuses `--doc` with --no-run and with a target selector.
    doc_only = "--doc" in flags
    flags = [f for f in flags if f != "--doc"]
    selected = any(f.split("=", 1)[0] in TARGET_FLAGS for f in flags)
    items = [] if doc_only else list_executables(flags, root, env)
    if not items and not doc_only:
        print("test-shard: no test executable found", file=sys.stderr)
        return 1
    failed, ran_exes, kept_total = [], 0, 0
    for item in items:
        e = exe_env(item, env, libtest)
        rc, out = run([item["exe"], "--list"], item["dir"], e)
        names = listing(out) if rc == 0 else None
        if names is None:
            # It cannot filter by name: the whole executable, on one shard.
            if shard_of([], "bin:" + item["name"], shards) != shard:
                continue
            cmd = [item["exe"]] + filters + libtest
            kept = None
        else:
            mine = [n for n in names if shard_of(lists, n, shards) == shard
                    and all(f in n for f in filters)]
            if not mine:
                continue
            cmd = [item["exe"]] + ["--exact"] + mine + libtest
            kept = len(mine)
        rc, out = run(cmd, item["dir"], e, exe_limit())
        emit(f"     Running {item['src']} ({item['exe']})")
        emit(out)
        ran_exes += 1
        kept_total += kept or 0
        if rc != 0:
            emit(f"error: test failed, to rerun pass `-p {item['pkg']} --test {item['name']}` (exit status {rc})"
                 + (f" (timed out after {exe_limit():g} s)" if rc == 124 and exe_limit() else ""))
            failed.append(item["name"])
    # Doc tests, with cargo's own run so the lines are cargo's.
    docs = None
    if not selected:
        rc, out = run(flags + ["--doc"] + filters + ["--", "--list"], root, env)
        docs = listing(out) if rc == 0 else None
    if selected:
        doc_cmd = []
    elif docs is None:
        doc_cmd = [] if shard_of([], "doc-tests", shards) != shard else \
            flags + ["--doc"] + filters + (["--"] + libtest if libtest else [])
    else:
        mine = [n for n in docs if shard_of(lists, n, shards) == shard]
        doc_cmd = flags + ["--doc"] + filters + ["--", "--exact"] + mine + libtest if mine else []
        kept_total += len(mine)
    if doc_cmd:
        rc, out = run(doc_cmd, root, env, exe_limit())
        emit(out)
        if rc != 0:
            failed.append("doc tests")
    print(f"test-shard: shard {shard} of {shards}: {ran_exes} executables, {kept_total} listed tests, "
          f"{len(failed)} failed" + (": " + ", ".join(failed) if failed else ""))
    sys.stdout.flush()
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
