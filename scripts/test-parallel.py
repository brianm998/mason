# SPDX-License-Identifier: GPL-3.0-or-later
"""Run a cargo workspace's test executables several at a time.

    python3 -I scripts/test-parallel.py [--jobs N] -- <cargo test command and args>

`cargo test` runs the test executables one after another; libtest runs the tests of one executable
in parallel, but never two executables at once. This lists the executables with
`<cargo test ...> --no-run --message-format=json`, runs them N at a time (default 4, or
MASON_TEST_JOBS), runs the executables named in `.mason/serial-tests.txt` alone afterwards, then runs
the doc tests with `<cargo test ...> --doc`.

Each executable's whole output is printed as one block, headed by cargo's own
`     Running <src> (<exe>)` line, so a parser that reads cargo's lines (`test X ... ok`) works on
it unchanged. A one-line summary closes the output. The exit status is 0 only when every executable
and the doc tests passed and at least one executable was found.

Name filters after the cargo flags (and everything after a second `--`) go to every executable.
Each executable runs in its package's directory with CARGO_MANIFEST_DIR set, as cargo does.
"""

import argparse
import json
import os
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor

# `cargo test` flags that take a value in the next argument.
VALUE_FLAGS = {
    "-p", "--package", "--exclude", "--target-dir", "--manifest-path", "--features", "-F",
    "--test", "--bin", "--example", "--bench", "-j", "--jobs", "--target", "--profile",
    "--message-format", "--color", "--config", "-Z", "--lockfile-path", "--timings",
}

SERIAL_FILE = ".mason/serial-tests.txt"


def split_cargo(argv):
    """(cargo flags, name filters, libtest args) from the words after our own `--`."""
    flags, filters, libtest = [], [], []
    if "test" not in argv:
        raise SystemExit("test-parallel: the command must be a `cargo test` command")
    at = argv.index("test")
    flags = list(argv[: at + 1])
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
        if a.startswith("-"):
            flags.append(a)
        else:
            filters.append(a)
        i += 1
    return flags, filters, libtest


def read_serial(root):
    names = set()
    try:
        with open(os.path.join(root, SERIAL_FILE), encoding="utf-8") as f:
            for line in f:
                line = line.split("#", 1)[0].strip()
                if line:
                    names.add(line)
    except OSError:
        pass
    return names


def list_executables(flags, root):
    """Build the tests and return [{name, src, exe, dir, pkg}] from cargo's JSON messages."""
    cmd = flags + ["--no-run", "--message-format=json-render-diagnostics"]
    p = subprocess.Popen(cmd, cwd=root, stdout=subprocess.PIPE, stderr=sys.stderr, text=True)
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
        kinds = target.get("kind", [])
        # As cargo's own header: the source path relative to the package.
        pkg_dir = os.path.dirname(m["manifest_path"])
        try:
            src = os.path.relpath(target["src_path"], pkg_dir)
        except ValueError:
            src = target["src_path"]
        if any(k in ("lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro", "bin") for k in kinds):
            src = "unittests " + src
        pkg = m.get("package_id", "").split("#")[-1].split("@")[0].split("/")[-1].split(" ")[0]
        found.append({"name": target["name"], "src": src, "exe": exe, "dir": pkg_dir, "pkg": pkg})
    rc = p.wait()
    if rc != 0:
        raise SystemExit(f"test-parallel: `{' '.join(cmd)}` failed with status {rc}")
    return found


def run_one(item, filters, libtest):
    env = dict(os.environ)
    env["CARGO_MANIFEST_DIR"] = item["dir"]
    if item["pkg"]:
        env["CARGO_PKG_NAME"] = item["pkg"]
    cmd = [item["exe"]] + filters + libtest
    start = time.monotonic()
    try:
        p = subprocess.run(
            cmd, cwd=item["dir"], env=env, stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        )
        out, rc = p.stdout.decode("utf-8", "replace"), p.returncode
    except OSError as e:
        out, rc = f"error: could not run {item['exe']}: {e}\n", 127
    return rc, out, time.monotonic() - start


_print_lock = threading.Lock()


def emit(item, rc, out, secs):
    head = f"     Running {item['src']} ({item['exe']})\n"
    with _print_lock:
        sys.stdout.write(head + out + ("" if out.endswith("\n") or not out else "\n"))
        if rc != 0:
            sys.stdout.write(f"error: test failed, to rerun pass `-p {item['pkg']} --test {item['name']}` "
                             f"(exit status {rc})\n")
        sys.stdout.flush()


def main():
    ap = argparse.ArgumentParser(add_help=True, usage="test-parallel.py [--jobs N] -- <cargo test command>")
    ap.add_argument("--jobs", type=int, default=int(os.environ.get("MASON_TEST_JOBS", "4") or "4"))
    ap.add_argument("cargo", nargs=argparse.REMAINDER)
    args = ap.parse_args()
    words = args.cargo[1:] if args.cargo[:1] == ["--"] else args.cargo
    if not words:
        ap.error("give the cargo test command after --")
    flags, filters, libtest = split_cargo(words)
    root = os.getcwd()
    started = time.monotonic()
    items = list_executables(flags, root)
    if not items:
        print("test-parallel: no test executable found", file=sys.stderr)
        return 1
    serial_names = read_serial(root)
    parallel = [i for i in items if i["name"] not in serial_names]
    serial = [i for i in items if i["name"] in serial_names]
    # Biggest executables first: they are the likely long ones, so they start early.
    parallel.sort(key=lambda i: -os.path.getsize(i["exe"]) if os.path.exists(i["exe"]) else 0)
    failed, ran = [], 0

    def job(item):
        rc, out, secs = run_one(item, filters, libtest)
        emit(item, rc, out, secs)
        return item, rc

    with ThreadPoolExecutor(max_workers=max(1, args.jobs)) as pool:
        for item, rc in pool.map(job, parallel):
            ran += 1
            if rc != 0:
                failed.append(item["name"])
    for item in serial:
        _, rc = job(item)
        ran += 1
        if rc != 0:
            failed.append(item["name"])
    # Doc tests: cargo's own run, so that the count includes them.
    doc = subprocess.run(
        flags + ["--doc"] + filters + (["--"] + libtest if libtest else []),
        cwd=root, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
    )
    sys.stdout.write(doc.stdout.decode("utf-8", "replace"))
    if doc.returncode != 0:
        failed.append("doc tests")
    secs = time.monotonic() - started
    print(f"test-parallel: {ran} executables ({len(serial)} serial) and doc tests, "
          f"{len(failed)} failed, {secs:.0f} s, {max(1, args.jobs)} at a time"
          + (": " + ", ".join(failed) if failed else ""))
    sys.stdout.flush()
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
