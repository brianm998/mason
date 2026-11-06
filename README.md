# MASON

**Multi-agent Automation and Session Orchestration Network.** A Rust daemon
(`masond`) and CLI (`mason`) that run coding agents through a configurable
pipeline of exploring, developing, testing, review and landing, for any number
of projects at once, with SwiftUI and Compose monitors.

Status: **pre-alpha, Stage 14**: tasks land in batches (one integration branch per batch, the suites run once over the union, a red is blamed on the task that caused it, a red post-land tier freezes landing), pipelines are defined in `.mason/mason.toml`, one scheduler orders every contended resource, test verdicts are proven from output, a task that needs you asks a short decision card, and two monitors (SwiftUI in `clients/swift`, Compose in `clients/compose`) draw the daemon's server-computed views over one WebSocket and are held in parity by a machine check (`masond --demo` plays a scripted scenario).
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
