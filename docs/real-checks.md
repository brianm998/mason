# Real checks

Several features were written and tested only against fakes (stage notes s20, s22, s23, s24 and
s25 say so). Their stages count as done in `clients/features.toml`, but a break against the real
thing would go unnoticed. This page lists the checks that touch the real thing, so that you can run
them, one by one, and record what you saw.

**You run these. An agent never does:** they spend tokens, use a real account, send mail or open a
port on your network. Each script that costs something refuses to start unless you set
`MASON_REAL_APPROVED=yes`, and each ends with one line that starts with `RESULT sNN-real:`. Paste
that line into the Results section at the bottom, with anything you saw that the script cannot check.

`bash scripts/exit/s299.sh` checks that every `scripts/exit/*-real.sh` is named on this page.

## Codex against the real CLI (s20)

- **What was never run:** the Codex adapter's flags and parser against the real `codex` binary
  (`docs/stages/s20.md`, "Nothing here proves it works with the real CLI").
- **Run:** `MASON_REAL_APPROVED=yes MASON_REAL_LIVE=yes bash scripts/exit/s299-real.sh`
- **Costs:** one `codex exec` call with a one-line prompt: a few tokens on your Codex/OpenAI
  account. Needs `codex`, signed in. Nothing is pushed or sent.
- **Look at:** the printed event types against `crates/adapters/fixtures/codex`; if they differ,
  the parser is wrong and a task should be filed.
- **Record:** the RESULT line under Results.

## Email through a real SMTP server, and speech (s23)

- **What was never run:** the SMTP channel with STARTTLS and login against a real server, and the
  speech command on a real machine (`docs/stages/s23.md`).
- **Run:** `MASON_REAL_APPROVED=yes MASON_REAL_SMTP=host:587 MASON_REAL_SMTP_FROM=... MASON_REAL_SMTP_TO=... MASON_REAL_SMTP_USER=... MASON_REAL_SMTP_PASSWORD_FILE=... MASON_REAL_SPEECH=say bash scripts/exit/s23-real.sh`
  (the other `MASON_REAL_*` channels, ntfy and a webhook, are optional; see the script's header).
- **Costs:** one real email from your account, and a push if you add ntfy. No tokens.
- **Look at:** the email arrives, its headers carry no password, and the sentence is spoken once.
- **Record:** the RESULT line, and whether the mail arrived.

## Sounds heard on a real device (s23)

- **What was never run:** alert sounds and sentences heard by a person.
- **Run:** `bash scripts/exit/s23-manual.sh`, then open a monitor (SwiftUI or Compose) at the
  printed address and follow the numbered list in the script's header.
- **Costs:** nothing; a scratch daemon on a free port. Needs speakers and a person.
- **Record:** which of the listed items you heard, under Results (this script prints no RESULT
  line because the checking is yours).

## The graph editor driven by its controls (s18)

- **What was never run by hand:** the editor's controls clicked in a real window. Since s299 both
  clients have offscreen tests that call the controls' actions (add step, connect, edit a field,
  undo) and compare the draft with the model (`bash scripts/exit/s299.sh`); a click in the real
  window is still yours.
- **Run:** no script. Open the monitor on a scratch project, go to Pipeline, and check by hand:
  1. select a step, press "+ test" in the palette: a new step appears after it;
  2. set "On success" of the selected step to the new step: an edge is drawn;
  3. type a value into a form field in the inspector and press Return (or Set): the node's summary
     changes;
  4. press Undo three times: the graph returns step by step to where it began; Redo goes forward;
  5. drag a node: it stays where you dropped it.
- **Costs:** nothing; use the demo daemon (`masond --demo`) or a scratch project.
- **Record:** pass or fail for each of the five, for each client, under Results.

## The device sample app (s25)

- **What was never run:** `docs/examples/device-sample` built and run (`docs/stages/s25.md:67`).
- **CI builds it** (jobs `device-sample-ios` and `device-sample-android`; build only, nothing is run).
- **Run on your machine:** `bash scripts/exit/s25-real.sh` (reads the real `xcrun` and `adb` and
  prints what MASON sees; boots nothing). Then follow `docs/examples/device-sample/README.md` to
  run its tests on a simulator, an emulator and a phone.
- **Costs:** nothing; needs Xcode, the Android SDK or a device.
- **Record:** the RESULT line, and for each device kind whether the sample's suite ran on it.

## The review and live-adapter jobs against a real model (s22)

- **What was never run:** the scheduled `cargo-mutants` run and the scheduled live adapter run
  (`docs/stages/s22.md`).
- **Run:** `MASON_REAL_APPROVED=yes bash scripts/exit/s22-real.sh`; add `MASON_REAL_LIVE=yes` for
  the live adapter job.
- **Costs:** the mutants run takes long (needs `cargo-mutants`). The live run calls the signed-in
  `claude` CLI and spends tokens.
- **Look at:** the second run must file nothing the first did not.
- **Record:** the RESULT line.

## Real phone pairing over Wi-Fi (s24)

- **What was never run:** pairing a real iOS or Android app with a daemon on the LAN
  (`docs/stages/s24.md:36`).
- **Run:** `MASON_REAL_APPROVED=yes bash scripts/exit/s24-real.sh` (add `VIEWER=1` for the viewer
  role), then follow the numbered list in the script's header on the phone.
- **Costs:** opens a TLS port on your Wi-Fi and advertises it over mDNS until you press Return.
  Needs the phone.
- **Record:** the RESULT line, and whether the phone found, paired and was refused as a viewer.

## The Android phone app on a real phone (s469)

- **What was never run:** the Android phone app on a real phone against a daemon.
- **Run:** `bash scripts/exit/s469-real.sh` prints the 8-step checklist; needs a phone on the same Wi-Fi.
- **Costs:** nothing.
- **Record:** the RESULT line, and pass or fail per step, under Results.

## The other real scripts

These earlier checks also use something real. They are on this page so that the list is complete.

- **s06-real.sh:** lands one task with the real Claude Code on a scratch repository (a few cents of
  Haiku). `CLAUDE_BIN=/path/to/claude bash scripts/exit/s06-real.sh`
- **s12-real.sh:** the monitor's answer path against a real daemon; no model. Needs macOS and
  Swift. `bash scripts/exit/s12-real.sh`
- **s15-real.sh:** `direct-ci` and `pr-batch` against a scratch GitHub repository: pushes branches,
  opens and merges pull requests. `MASON_REAL_REPO=owner/scratch MASON_REAL_APPROVED=yes GH_TOKEN=... bash scripts/exit/s15-real.sh`
- **s17-real.sh:** MASON lands a change to its own source in a scratch copy and updates itself;
  builds in release mode (minutes); no model. `bash scripts/exit/s17-real.sh`
- **s1153-real.sh:** the real `claude` CLI, given a plugin directory and the settings MASON writes,
  lists the plugin's skill and not the working directory's; spends no tokens (closed API port,
  temporary HOME). Needs the `claude` CLI. `MASON_REAL_APPROVED=yes bash scripts/exit/s1153-real.sh`
- **s26-real.sh:** `mason init` against a real GitHub remote with the real `claude` CLI; needs a
  scratch repository and a token. `S26_REPO=owner/scratch bash scripts/exit/s26-real.sh`
- **s40-real.sh:** approve and reject through both monitors against a real daemon; no model.
  Needs macOS and Swift. `bash scripts/exit/s40-real.sh`

Record each RESULT line under Results.

## Results

Paste one line per run, newest first: the `RESULT sNN-real:` line, then who ran it and anything the
script cannot check.

(none yet)
