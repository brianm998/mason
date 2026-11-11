# 0119: Migrating star: the inventory, and what the owner must decide before Step 2

Stage 27, Step 1. The inventory itself is in `docs/stages/s27.md`; this record lists what the design
left open and what the starpipe code showed. **Step 2 (landing `.mason/` in star through starpipe) does
not start until the owner has answered each decision below.** Nothing was installed and star's
repository was not touched. Star's files were read from `origin/develop` (8bbf5d9).

Each decision has one recommendation, marked *Recommended*. The owner's answer is recorded under
"Answer" when it comes; until then every line says "open". All seven were answered on 2026-11-11: the owner accepted every recommendation.

## 1. The per-ticket Gherkin feature runs (`gherkin.yml` with a `features` input)

starpipe dispatches `gherkin.yml` on a batch branch with `{"features": "a,b"}` for each ticket whose plan
names Gherkin features (`ship.py` 169-175, `train.py` 444-500) and waits for that run. MASON's direct-ci
`trigger` posts `{"ref": ...}` for one workflow only (`crates/adapters/src/github.rs`, `trigger`).

- **A (Recommended): a script step.** A `command` step after `test` runs
  `gh workflow run gherkin.yml --ref <candidate> -f features=<list>` and waits with `gh run watch`,
  failing on red. The feature list comes from the task's plan. It keeps today's coverage, needs no MASON
  change, and costs one script (`.mason/scripts/gherkin-features.sh`, not yet written: it needs the plan's
  field name from a real ticket, and `gh` credentials for MASON's environment).
- B: drop it. Only the nightly matrix runs the features. Cheapest; a feature regression shows up a day later.
- C: teach direct-ci to dispatch extra workflows with inputs. The right long-term shape, but it changes
  landing code (a guard file), so it is its own task and the owner's approval.

Answer (owner, 2026-11-11): **A**, a script step. `gherkin-features.sh` is still to be written; it waits for the shadow period to show where a MASON task carries its feature list.

## 2. `macos-gate.yml` on the batch

The task text said pr-gate on a dispatched ref "covers what `macos-gate.yml` checks". **That is only half
true.** `macos-gate.yml` runs (a) pr-gate's macOS build and StarCore tests and (b) `gherkin.yml` in smoke
mode on every macOS leg: build-macos, cli-macos, desktop-macos and mac-gui (its own header, lines 7-9).
pr-gate on `workflow_dispatch` runs (a) (see decision 6) and not (b); (b) otherwise runs only on the push
to `develop` (`gherkin-smoke.yml`).

- **A (Recommended): accept (a) and drop the dispatch; keep (b) post-merge.** A macOS-only break in the
  smoke legs then shows up as a red `develop` rather than a held batch. Same as a PR's normal behaviour today.
- B: a second script step like decision 1, dispatching `macos-gate.yml` and waiting. Keeps (b) before landing.
- C: extend pr-gate itself to include the smoke legs on dispatch. A change to star's workflow, outside this task.

Answer (owner, 2026-11-11): **A**. No `macos-gate.yml` dispatch before landing; the macOS smoke legs stay post-merge.

## 3. Hosted macOS runners that never start

starpipe lets a batch land with a note after `macos_gate_wait_s` (default 3 h) when the macOS jobs got no
runner, after one rerun (`config.py` 88-93, `train.py` 566-576). `macos_gate_hard` holds instead. MASON keeps
the run Pending until the CI step's `timeout_s` and then the landing attempt ends with the error "still not finished after N s" (`landing.rs`
`ask`, `ci_verify`); it never lands past it. The draft registration sets `timeout_s = 14400`.

- **A (Recommended): keep MASON's rule, and pick a timeout.** Never landing without the macOS result is the
  safer default and needs no code. The cost is a batch parked when GitHub has no macOS runner for hours; the
  owner then re-runs or answers it.
- B: land with a note after N hours. This changes a verdict (a green that is not green), so it needs MASON
  code in the landing guard file and the owner's explicit approval as a guard change. Not recommended until
  A has been seen to hurt in the shadow period.

Answer (owner, 2026-11-11): **A**, keep MASON's rule and never land without the macOS result. Timeout: the draft's 4 h (`timeout_s = 14400`) stands.

## 4. Forcing the CPU merge in every suite (the GPU rule)

`adapter_star.py` never sets `useGPUForMerge`. Reading star's code (origin/develop):
- `Config.useGPUForMerge` defaults to `GPUCapability.hasFullAcceleration()`, true whenever Metal finds a
  device (`StarCore/Sources/StarCore/Config.swift` 387; `GPUCapability.swift`).
- `PixelatedImage` falls back to `true` when it has no config (`PixelatedImage.swift` 1038).
- The `star` command line has `--no-use-gpu` (`cli/Sources/star/StarCli.swift` 260-268, `ConfigOverrides.swift` 108).
- Two tests set it false by hand (`DirtyCleanMethodTests.swift` 81, `FrameFormatMismatchTests.swift` 26).
- **There is no environment variable, and nothing reads `~/.star.userprefs.json` for it.** star's
  `CLAUDE.md` says "config / `~/.star.userprefs.json` / test setup", but the only reader of that file is the
  gui's own preferences (`gui/star/UserPreferences.swift`); the config key is read from `config.json`. So a
  temporary `HOME` holding that file would do nothing. (Corrects the first plan for this task.)

So today a suite can only avoid the GPU merge if each test sets it, and most do not. The draft sets
`STAR_USE_GPU_FOR_MERGE=0` on every suite as the explicit key, and the exit script checks for it, **but star
ignores that variable until a star change makes `Config` read it** (and `PixelatedImage` stop defaulting to
true).

- **A (Recommended): a star ticket, filed through starpipe like Step 2**: `Config.init` reads
  `STAR_USE_GPU_FOR_MERGE` (any value but "1" means off), `PixelatedImage`'s fallback becomes `false`, and
  a test pins it. Then the draft's key is real. This lands before MASON runs any star suite.
- B: audit the tests and have each set the flag in its own setup. Slow, and a new test can forget it.
- C: hide Metal from the test processes (for example an environment that makes `MTLCreateSystemDefaultDevice`
  return nil). Not found in star's code and not verified to work; not recommended.

Until A lands, **no MASON suite may run on this Mac**, because the key is only a request. Answer (owner, 2026-11-11): **A**. Filed through starpipe as brianm998/starry_timelapse_airplane_remover#544: `STAR_USE_GPU_FOR_MERGE=0` gates every `ImageAligner` merge and warp entry point (which covers `PixelatedImage`'s `?? true` fallback without changing it) and clears `Config.useGPUForMerge` on default and decode. The `.mason/` ticket, #545, is blocked by it.

## 5. Freezing, after the switch

starpipe's `freeze` holds every heavy-lock file, so its daemon, the agents in its worktrees and wrapped
session commands all wait (`pause.py` 1-8, `cli.py` 63-76). During the shadow period it also stops MASON's
suites, because they go through the wrapper. After the switch:

- **A (Recommended): `mason pause --mode all`, plus an exclusive hold of MASON's machine classes for the
  length of the freeze** (`mason run --exclusive -- sleep <seconds>`, which takes the whole machine).
  `pause` stops MASON starting the next step; the hold stops anything that goes through `mason run` (suites
  and an agent's hooked commands), and a command already running is not stopped by either. **An agent's own
  commands are held only if it is run with MASON's Bash hook**, which asks and does not forbid; a raw
  `swift build` from a session without the hook is not held. starpipe's freeze had the same limit for old code
  and covered it by also holding the old one-slot file.
- B: a small `mason freeze` command doing both. A MASON change; propose it after the switch if A proves clumsy.

Answer (owner, 2026-11-11): **A**, `mason pause --mode all` plus an exclusive hold of the machine classes.

## 6. `pr-gate.yml` on a ref with no PR

On `workflow_dispatch` the `changes` job leaves `program=true` and `macos=true` (pr-gate.yml 69-71; only the
`pull_request` branch narrows them), so every job runs, including both macOS jobs. Nothing breaks: it is
slower than a narrowed PR run and it covers what a PR run skips. **No change to star is proposed.**

- **A (Recommended): leave it.** Landing is a batch, and a batch always wants everything.
- B: let `changes` also skip on a dispatch that carries only docs or pipeline paths. A change to star's workflow.

Answer (owner, 2026-11-11): **A**, leave `pr-gate.yml` as it is.

## 7. The deep tier

The draft's agent profiles are light (explore, develop) and a lighter-effort review. starpipe uses Sonnet unless
the owner allows Opus (`allow-opus`, `model_<stage>`).

- **A (Recommended): no deep tier at first.** Add `[agent.deep] tier = "deep"` and point `develop` at it later.
- B: deep for `develop` only, from the start.

Answer (owner, 2026-11-11): **A**, no deep tier at first.

## Not decisions, but the owner should know

- **Dropped-line audit after conflict resolution** (`landing.py` `audit`): MASON has none; it is task #34. The
  switch waits for it; shadow does not.
- **Infrastructure reruns.** starpipe reruns only reds that look like infrastructure; MASON's `rerun-failed`
  rung reruns any red CI once (whole run, fresh candidate ref). Slower for a real red, no worse for a flake.
- **MASON's impact map unions its entries and an unmapped path selects every suite**, so a path cannot be taken
  out of `StarCore/**`. A localization-only change runs `test-StarCore` as well as the i18n check, where starpipe
  ran only the check. Stricter, slower, and recorded in the draft.
- **Draft counts.** `min_tests` is starpipe's real count only for StarCore (1,625), daemon (314) and StarCpp (254);
  every other suite is 1, marked `UNMEASURED`, until the shadow runs give a number.
