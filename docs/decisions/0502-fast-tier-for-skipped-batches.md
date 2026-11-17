# 0502: a batch whose testing was skipped runs a fast unit tier

**Decision.** When every task in a landing batch has its test step skipped, the batch is verified by the build floor and then by each selected suite's `fast_command`, declared in `.mason/mason.toml` (the line for MASON's cargo suite follows once the running daemon knows the key; see s509). The full run stays owed, as the skip debts say. After a moved base the batch is rebuilt and the fast tier runs again; it is not fully re-tested (s429's rule).

**Why.** A rebuild compiles test code and runs none of it. On 2026-11-16 two tasks that were green alone merged into a red `matrix.rs`, and a test that takes 0.02 s never ran. The tier is declared in the project file, not in code, so each project names what is fast.

**Trust.** `crates/daemon/src/landing.rs` and `.mason/**` are in `PROTECTED`; this change waits for the owner's approval. It adds a check to the landing path and loosens nothing.
