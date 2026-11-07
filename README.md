# MASON

**Multi-agent Automation and Session Orchestration Network.** A Rust daemon
(`masond`) and CLI (`mason`) that run coding agents through a configurable
pipeline of exploring, developing, testing, review and landing, for any number
of projects at once, with SwiftUI and Compose monitors.

Status: **pre-alpha, Stage 20** (plus s41: the self-hold scan reads only what a task adds on the freshest base, judges new exit scripts as new, ignores cleanup `|| true`, and says why in one line with file and line): the owner answers a task's question and decides its approvals by clicking in either monitor (option buttons with the recommended one marked, an Other box that sends on Enter, arrow keys and Return; Approve with the exact diff hash shown and Reject with a reason for a proposal, an approval step or a self-hosting hold; the top bar's count opens the first task that needs you; a non-owner sees the card and no buttons), a second agent (the Codex CLI), a second build system (Jenkins) and a generic one (any trigger and status, each a command or a URL) pass the same conformance kits as the first, with agent profiles that retry up a ladder of models, plain git over ssh as a host with no features, and optional `sandbox-exec` / `bubblewrap` sandboxing; a project can add what it needs without forking (script steps with `MASON_*` variables and a JSON result, prompt packs, and process plugins in `.mason/extensions/` that speak versioned JSON-RPC over stdio in any language, add a step type with an editor form made from their `config_schema`, and run only after the owner approves their content hash), the pipeline can be edited on a canvas in both monitors (swimlanes, forms from JSON Schema, a test grid, the landing settings, a simulation, a diff and what it affects, undo and redo, apply) with the daemon keeping the TOML's comments, MASON can work on its own repository, with changes to its gates (or that weaken them) held for the owner by the running version's rules, the daemon updates itself from a channel without interrupting work (an immutable build proves itself on a copy of the database, takes over by `exec`, and rolls back by itself after three crashes), all seven landing modes (CI on candidate refs, pull requests, the host's merge queue; GitHub Issues, pull requests and Actions behind small adapter traits, tested on recorded responses and a fake host), tasks land in batches (one integration branch per batch, the suites run once over the union, a red is blamed on the task that caused it, a red post-land tier freezes landing), pipelines are defined in `.mason/mason.toml`, one scheduler orders every contended resource, test verdicts are proven from output, a task that needs you asks a short decision card, and two monitors (SwiftUI in `clients/swift`, Compose in `clients/compose`) draw the daemon's server-computed views over one WebSocket and are held in parity by a machine check (`masond --demo` plays a scripted scenario).
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
