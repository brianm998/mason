# 1069: the expected dump waits for trailing views

Decision: after reaching the target seq, `settle` in crates/conformance keeps draining messages until 300 ms pass with none, as Compose's tool does. This extends decision 1070: `should_leave` still decides when `settle` may leave, and its 5 s fallback for a target not reached is unchanged.

Why: views of one step arrive after the first, so returning at once dumped the previous step's rows. The decision is a pure function (`trailing_done`) so tests need no clock.
