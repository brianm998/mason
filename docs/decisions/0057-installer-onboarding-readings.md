# 0057: How the installer and `mason init` read the design

Stage 26 (Part 21). The design says what the conversation asks and not how the edges behave.
These are the readings taken, so they can be argued with.

1. **The flags.** One per question: `--tracker`, `--agent` (and `--agent-bin`), `--sessions`,
   `--workflow`, `--suites accept|skip`, `--base`, `--landing direct|direct-ci|pr|batched`,
   `--alerts LIST|none`, `--no-prove`. Question 1 (detect) has no flag of its own: `--yes`
   confirms it. Extras: `--id`, `--force`, `--ci-workflow yes|no`, `--prove-only`. A question is
   asked only on a terminal and only when its flag is absent. Without a terminal and without
   `--yes`, `init` stops and names the flag; it does not guess for someone who cannot answer.
   `--yes` takes the recommended answer to every question not flagged (local tracker, one
   session, solo-fast, accept suites, the detected branch, the workflow's landing, keep the
   built-in alerts).
2. **The branch is `mason/setup`, made in a temporary worktree, never pushed, never merged.**
   Part 21 says "on a branch". A worktree means the owner's checkout and branch are untouched;
   merging is the owner's act because `.mason/` runs with the pipeline's authority (Part 12:
   only the base branch's `.mason/` counts). Cost: a first task needs the merge, a push when
   there is a remote, and a daemon restart (projects are read at start), so `init --prove-only`
   is a separate step. `init` prints the three commands.
3. **Templates are MIT** (`templates/*`, enforced by `check-spdx.sh`; Part 22 and LICENSING.md).
   They are embedded in the binary with `include_str!` from the repository path. That works for
   building here; `cargo publish` at Stage 31 must either move them into the crate or vendor
   them. Not solved here.
4. **Pull-request workflows without a GitHub remote fall back to `direct`** and say so; an
   explicit `--landing pr` without one is refused. `pr` and `batched` write a `[host]` section
   with `token = "env:GITHUB_TOKEN"`: a reference, never a value.
5. **Suites.** Proposed per language MASON can read (Rust and Go by regex, Python by
   unittest regex or pytest, Swift by xctest). Each is run once in the owner's working tree and
   judged as the pipeline judges. A suite that finds no tests is dropped (a vacuous suite would
   turn every task red); a red suite is kept and reported. The baseline is written to
   `.mason/baselines.toml` for people: the pipeline does not read it yet. With no suite the
   test step runs `true` and `init` says plainly that it proves nothing.
6. **Install verifies a SHA-256 and says signing is not verified.** There is no release or
   signing key, so claiming more would be false. Signature checking is wired for `minisign` plus a
   pinned public key and untested for lack of one.
7. **`mason adopt` never writes `.mason/`.** The plan said "refuses to overwrite an existing
   `.mason/`". Read as: adopting registers a project that has one, writes only the overlay, and
   leaves `.mason/` byte for byte as it was; with none it refuses and points at `init`.
8. **`mason remove` needs `--yes`**; without it, it lists and exits 2. It deletes the overlay and
   `.mason/` in the working tree, removes the Filing section only if it is exactly what `init`
   wrote (an edited one is the owner's), and leaves tracker tasks, branches and machine-wide
   files, saying so.
9. **The human-gated workflow's "tasks start as proposed"** is a property of how tasks are
   filed (`mason task add --proposed`, or `[tracker] import_as = "proposed"`, which is the
   default for a GitHub tracker), not of the definition. The template has the two approval
   steps; tasks that arrive already explored start at `develop`.
10. **Alerts.** Only `sound` and `speech` are written by `init`; `email` and `push` need an
    account and a secret, so it names the section to add instead of writing a route to a channel
    that cannot send.
