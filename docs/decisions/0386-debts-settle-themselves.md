# 0386: open debts close on a later green and run when idle

**Decision.** A skip debt closes by itself when every suite of its scope has a recorded green at a base that contains the commit its task landed; the proof is stored on the debt and shown by `mason debt list`. Debts still open are run once each through `pay` while the compile queue is empty; only a debt a run found red raises the "debts" alert.

**Why.** 82 debts were open on 2026-11-14 and paying each needed a manual command, though most suites had been green on develop since. A green at a base without the task's commit proves nothing about the task, so ancestry is required; a debt whose landed commit is unknown stays open.

**Trust.** `crates/daemon/src/suites.rs` is in `PROTECTED`; the change is there because `pay`, `after_landing` and the green record live there. It closes a debt only on recorded proof and never weakens a gate: no suite is skipped, and a red run still leaves the debt open.
