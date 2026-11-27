<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1215: a suite a node gives back stays off the coordinator

A project in `remote` mode with fallback none must never run tests here. Checks already obey this;
suites did not, so losing a node started `cargo test --workspace` on the coordinator. Suites now
share the checks' rule (`hostedcheck::stays_off_for`) and are placed again off-machine.

Why a protected file (`suites.rs`, verdict logic) changes: the gap is in where a suite runs. No
verdict changes: an unplaced suite is an outage, never a red, a pass or a skipped suite.
Owner approval is needed.
