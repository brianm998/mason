# MASON

![MASON mascot banner](docs/images/readme_banner.png)

**Multi-agent Automation and Session Orchestration Network.** A Rust daemon
(`masond`) and CLI (`mason`) that run coding agents through a configurable
pipeline of exploring, developing, testing, review and landing, for any number
of projects at once, with SwiftUI and Compose monitors.

## Status: alpha

MASON runs its own development every day and also runs other projects, but it
has no published release yet and a small user base.

- **What works:** everything under [What it does](#what-it-does). MASON runs as
  its own pipeline (see [docs/self-hosting.md](docs/self-hosting.md)).
- **What is missing:** no release has been published yet, so
  [docs/install.md](docs/install.md) means building from source until one is.
  Release automation exists ([docs/RELEASING.md](docs/RELEASING.md)), but its
  signing and notarising steps and the iOS TestFlight upload
  ([docs/ios-testflight.md](docs/ios-testflight.md)) need credentials that have
  not all been set up. There has been no security review. The App Store
  licence permission is still waiting for legal review.
- **What to expect:** configuration, the protocol and the stored data can still
  change without a migration path. Breaking changes are not announced.

The daemon targets macOS and Linux, with a Windows port in progress. The
monitors run on macOS (SwiftUI) and on macOS, Linux and Windows (Compose).

## What it does

- **Pipelines as files.** A project's pipeline lives in `.mason/mason.toml` and
  its landing rules in `.mason/landing.toml`. `mason init` takes a project from
  nothing to a landed first task, using one of the example workflows in
  [`templates/`](templates).
- **One scheduler for everything shared.** LLM sessions, compile and test
  slots, devices and disk are all ordered by one broker. It explains every wait
  and raises an alert when a wait looks stuck.
- **Test verdicts proven from output.** Suites, impact maps, minimum test
  counts and recorded flakes mean an empty or vacuous run cannot pass.
- **Landing.** Seven landing modes (`direct`, `direct-ci`, `local-only`,
  `local-only-ci`, `pr-each`, `pr-batch`, `host-queue`), from straight to a
  branch through CI on candidate refs and pull requests to the host's merge
  queue. Tasks land in batches: one integration branch per batch, the suites
  run once, a red is blamed on the task that caused it, and a red post-land
  tier freezes landing.
- **Remote and scheduled test runs.** Compile and test steps can run on GitHub
  Actions (a project's `[remote_runs]` section) or on other Macs that serve as
  nodes (see [docs/nodes.md](docs/nodes.md)). A `ci` schedule runs the full
  test at a set time or after a quiet spell, only when the base tip is not
  already fully tested.
- **Adapters.** Claude Code and Codex agents, with model ladders and usage
  limits per provider account. GitHub Issues, pull requests and Actions, Jenkins
  or any generic build system, and plain git over ssh. Device adapters for the
  iOS simulator and devices (`simctl`, `devicectl`) and Android (`adb`).
  Optional `sandbox-exec` or `bubblewrap` sandboxing.
- **Extending without forking.** Script steps, prompt packs, and process plugins
  in `.mason/extensions/` that speak versioned JSON-RPC in any language (see
  [docs/plugin-protocol.md](docs/plugin-protocol.md)). A plugin runs only after
  the owner approves its content hash.
- **Monitors.** SwiftUI (`clients/swift`) and Compose (`clients/compose`)
  desktop monitors draw the daemon's views over one WebSocket and are held in
  parity by a machine check. Both include a canvas pipeline editor and live step
  progress, and you answer a task's questions or approve its work by clicking.
  Phone clients exist for iOS (`clients/swift/ios`) and Android
  (`clients/compose/android`) and pair over the LAN.
- **Attention.** A task that needs you asks one short decision card. Alerts are
  routed to the monitor, sound, speech and push, then go quiet once cleared or
  acknowledged. A watchdog speaks up when the daemon itself is down.
- **Self-hosting and self-update.** MASON works on its own repository, and
  changes to its own gates are held for the owner. The daemon updates itself
  without interrupting running work and rolls back by itself after repeated
  crashes. It also files proposals about its own problems and checks whether
  each fix helped.
- **Durability.** Every change is an event in a SQLite log, step hosts outlive
  the daemon, and `kill -9` mid-session loses nothing.

`masond --demo` plays a scripted scenario for the monitors. The design is in
[docs/DESIGN.md](docs/DESIGN.md), decisions in
[docs/decisions](docs/decisions), per-stage notes in
[docs/stages](docs/stages), and installation in
[docs/install.md](docs/install.md).

## Licence

GPL-3.0-or-later, see [LICENSE](LICENSE). A draft section 7 App Store
additional permission is in
[LICENSE-ADDITIONAL-PERMISSION](LICENSE-ADDITIONAL-PERMISSION) and is awaiting
legal review. See [docs/LICENSING.md](docs/LICENSING.md) for what the licence
does and does not cover, including generated output and templates.

The code is GPL, but the mascot character and its art are not: see the draft
brand notice, [NOTICE.md](clients/swift/Sources/MasonMascot/Art/NOTICE.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Every commit needs a DCO sign-off.
Report vulnerabilities privately, see [SECURITY.md](SECURITY.md).

## Layout

`crates/` Rust workspace, `clients/` monitors and phone clients, `schemas/`
protocol and definition schemas, `templates/` starter workflows, `prompts/`
default prompt packs, `packaging/` Homebrew and Windows packaging, `site/` the
download page, `spikes/` early experiments, `docs/` design, decisions, spikes
and stage reports, `scripts/exit/` per-stage exit tests.
