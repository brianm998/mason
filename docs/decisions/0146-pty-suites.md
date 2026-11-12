# 0146: suites and command steps under a pseudo-terminal (`pty = true`)

Task #167. Star's starpipe runs `swift test` and `xcodebuild test` under a pseudo-terminal so
XCTest writes line by line; under a pipe it block-buffers, and a hung test's name appears only
when the process exits. This adds the same to MASON, off by default.

## Decisions

- **The key.** `pty` (bool, default false) on `[suite.NAME]` and on a command or test step.
  A non-bool is a definition problem naming the key. It changes how output is captured, never
  a verdict, a parser, an exit code or a limit.
- **The wrapper is macOS-only**: `script -q /dev/null sh -c '<quoted command>'`. BSD `script`
  passes the child's exit status through. Off macOS the key is accepted and the command runs
  unwrapped (util-linux `script` takes different arguments and does not pass the status on by
  default; starpipe uses Python's `pty.spawn` there). The stage report says so.
- **`script`, not a pty library.** It is what starpipe uses on macOS and needs no new
  dependency in the daemon. The step host's SIGTERM to the process group still reaches the
  command (`script` forwards it); a test checks that no command is left behind.
- **Device substitution first, wrapping second**, at every place a suite's command runs: the
  suite run, the rerun of failures, the baseline run and the new-tests-at-base run (the
  negative control). One flag, one behaviour.
- **The log keeps the raw bytes; the progress line is cleaned.** For a pty stream only, ANSI
  CSI/OSC sequences and carriage returns are removed from the lines pushed to monitors. The log
  file and the parsers' input are untouched (on macOS `script` begins its output with `^D` and
  two backspaces when its stdin is at its end; the progress line drops them, the log keeps them), so a verdict cannot depend on the stripping.
- **"Current test" is the latest output line.** For a pty stream of a non-agent step, the
  latest non-empty cleaned line becomes the progress record's `message`, so `mason task show`
  and `--watch` show `Test Case 'X.y' started.` while it runs. No new parsing in `crates/testing`.

## Trust

`crates/testing/src/model.rs` (the `Suite.pty` field) and `crates/daemon/src/suites.rs` (the
command is wrapped at its run sites) are `PROTECTED`; they wait for the owner's approval. Neither
changes what is judged, only the command line and how output is captured.
