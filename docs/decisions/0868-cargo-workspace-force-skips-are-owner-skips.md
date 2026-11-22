<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0868: cargo-workspace force-skips are owner skips

## Context
Task #802 reported 18 force-skips of `cargo-workspace`. Debts with scope `suite:<name>` come only from owner skips (suites.rs:3918-3980) and Land now (suites.rs:5193-5212); the detector (introspect.rs:552-562) counts them all as suite trouble.

## Decision
Keep the cargo-workspace suite as it is. Treat the count as owner decisions, not failures.

## Consequences
The detector keeps reporting until its counting changes (proposal filed). The reason text of the stored debts was not checked.
