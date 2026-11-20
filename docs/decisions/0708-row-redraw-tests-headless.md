# 0708: row-redraw tests stay strict on a headless runner

Status: accepted

The runner evaluated no RowView bodies for the offscreen host. We make the host perform a real
display cycle (no window ordered in, so nothing appears on screen) rather than skip the tests or
loosen the `> 0` preconditions, which would weaken the gate. No timing assertions are used; the
run-loop turns only give the display cycle a chance to run. If this proves insufficient, the tests
need a different harness, not a skip.
