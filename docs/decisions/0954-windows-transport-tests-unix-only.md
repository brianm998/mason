<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0954: the step-host transport tests are Unix-only for now

**Cause.** `tests/portable.rs` ran on Windows, where `mason-platform`'s `net.rs` is a stub whose `bind` always returns Unsupported. The host exited before writing `host.kv`, so all three tests failed with "step host exited before it was up". The build-side items in the ticket (`reject_first`, `zombie_children`) were already gated on `develop`.

**Decision.** `portable.rs` is `#![cfg(unix)]`, like `host.rs`.

**Alternatives.** Implement the Windows named-pipe transport now (the IPC port task's work, too large here). Ignore each test on Windows (hides them in the output).

**Coverage cost.** Connect, reattach and stop through the transport are tested on Unix only until the Windows IPC port lands and removes the gate.
