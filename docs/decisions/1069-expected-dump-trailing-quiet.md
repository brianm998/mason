# 1069: the expected dump waits for trailing views

Decision: `settle` in crates/conformance keeps draining messages until 300 ms pass with none after reaching the target seq, as Compose's tool does.

Why: views of one step arrive after the first, so returning at once dumped the previous step's rows. The decision is a pure function (`done`) so tests need no clock.
