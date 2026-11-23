# 1005: a replayed run_check is the same call

A `run_check` has no effect of its own, so `mason-mcp` sends it again when the controller goes away. The call now has an id (`call_id`, made once per tool call, resent unchanged). Results of a call with an id are "delivered" when the reply is written to the contract socket, not when the in-process loop reads them, so a replay with the same tree and command takes them. The base is folded only on a call whose id is not the one recorded for the run; a replay runs on the tree it began on and says if the base has moved.

A call with no id (an older `mason-mcp`, a direct call in a test) delivers at once, as before.

Test steps need no change: evidence is recorded per tree and suite as each suite passes, and `recover` re-runs the step, which reuses it.

No gate, guard, budget or verdict logic is changed; `suites.rs` and the protected paths are untouched.
