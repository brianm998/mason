# MASON

**Multi-agent Automation and Session Orchestration Network.** A Rust daemon
(`masond`) and CLI (`mason`) that run coding agents through a configurable
pipeline of exploring, developing, testing, review and landing, for any number
of projects at once, with SwiftUI and Compose monitors.

<!-- status:begin -->
Status: **pre-alpha**. What each stage note says it built (generated from `docs/stages` by `scripts/gen-status.sh` when a change lands; do not edit between the markers):

- **s00**: Stage 0: Repository and ground rules
- **s01**: Stage 1: Spikes
- **s02**: Stage 2: Core domain and reducer
- **s03**: Stage 3: Event store
- **s04**: Stage 4: Step host
- **s05**: Stage 5: Agent runner and step contract
- **s06**: Stage 6: Walking skeleton
- **s07**: Stage 7: Definitions and configuration
- **s08**: Stage 8: Scheduler and resources
- **s09**: Stage 9: Testing system (completes M2)
- **s10**: Stage 10: Attention and overrides
- **s11**: Stage 11: Protocol, views and parity tooling
- **s12**: Stage 12: SwiftUI monitor (owner checkpoint)
- **s13**: Stage 13: Compose monitor (completes M3)
- **s14**: Stage 14: Landing engine, direct and local-only
- **s15**: Stage 15: GitHub, CI and PR landing
- **s16**: Stage 16: Self-reload and updates
- **s17**: Stage 17: Self-hosting and gate protection (completes M4)
- **s18**: Stage 18: Graph editor
- **s19**: Stage 19: Plugins and extensions
- **s20**: Stage 20: Second adapters
- **s40**: s40: Monitors answer and approve by clicking
- **s41**: s41: The self-hold scan flags only what a task really weakens
- **s42**: Task dependencies (s42): Task dependencies (GitHub issue #7)
- **s43**: MASON reserves each task's stage note, exit script and decision numbers when it is claimed, tells the task in its prompt, sends back a task that uses another number, and builds the README status block itself.
- **s44**: `per_week` in `[self]` counts agent and outside tasks only; the owner's own filings and tasks cancelled before any step started are not counted, and `mason self` shows the split.
- **s45**: every running step keeps a live progress record in the daemon (what it counts, done, total, last output, when it last moved), fed from its step host's output as it arrives, pushed to monitors as `StepProgress` (at most once a second for output alone) and carried by the views.
- **s55**: Fixed red CI on develop: s17.sh now expects the new hold wording and Compose Editor.kt captures maxWidth in the constraints scope.
<!-- status:end -->

Pre-alpha. Tasks can wait for others (`mason task add --after 3,4`, GitHub's "blocked by" and `Depends on #N` lines are read when issues are imported, in dependency order; a change on GitHub holds or releases a queued task; both monitors show "waiting for" and "blocks"), the owner answers a task's question and decides its approvals by clicking in either monitor (option buttons with the recommended one marked, an Other box that sends on Enter, arrow keys and Return; Approve with the exact diff hash shown and Reject with a reason for a proposal, an approval step or a self-hosting hold; the top bar's count opens the first task that needs you; a non-owner sees the card and no buttons), a second agent (the Codex CLI), a second build system (Jenkins) and a generic one (any trigger and status, each a command or a URL) pass the same conformance kits as the first, with agent profiles that retry up a ladder of models, plain git over ssh as a host with no features, and optional `sandbox-exec` / `bubblewrap` sandboxing; a project can add what it needs without forking (script steps with `MASON_*` variables and a JSON result, prompt packs, and process plugins in `.mason/extensions/` that speak versioned JSON-RPC over stdio in any language, add a step type with an editor form made from their `config_schema`, and run only after the owner approves their content hash), the pipeline can be edited on a canvas in both monitors (swimlanes, forms from JSON Schema, a test grid, the landing settings, a simulation, a diff and what it affects, undo and redo, apply) with the daemon keeping the TOML's comments, MASON can work on its own repository, with changes to its gates (or that weaken them) held for the owner by the running version's rules, the daemon updates itself from a channel without interrupting work (an immutable build proves itself on a copy of the database, takes over by `exec`, and rolls back by itself after three crashes), all seven landing modes (CI on candidate refs, pull requests, the host's merge queue; GitHub Issues, pull requests and Actions behind small adapter traits, tested on recorded responses and a fake host), tasks land in batches (one integration branch per batch, the suites run once over the union, a red is blamed on the task that caused it, a red post-land tier freezes landing), pipelines are defined in `.mason/mason.toml`, one scheduler orders every contended resource, test verdicts are proven from output, a task that needs you asks a short decision card, and two monitors (SwiftUI in `clients/swift`, Compose in `clients/compose`) draw the daemon's server-computed views over one WebSocket and are held in parity by a machine check (`masond --demo` plays a scripted scenario).
is in [docs/DESIGN.md](docs/DESIGN.md).

## Licence

GPL-3.0-or-later, see [LICENSE](LICENSE). A draft section 7 App Store
additional permission is in
[LICENSE-ADDITIONAL-PERMISSION](LICENSE-ADDITIONAL-PERMISSION) and is awaiting
legal review. See [docs/LICENSING.md](docs/LICENSING.md) for what the licence
does and does not cover, including generated output and templates.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Every commit needs a DCO sign-off.
Report vulnerabilities privately, see [SECURITY.md](SECURITY.md).

## Layout

`crates/` Rust workspace, `clients/` monitors, `schemas/` protocol and
definition schemas, `templates/` starter workflows, `prompts/` default prompt
packs, `docs/` design, decisions, spikes and stage reports, `scripts/exit/`
per-stage exit tests.
