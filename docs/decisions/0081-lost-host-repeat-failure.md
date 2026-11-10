# 0081: the repeated "step host was lost" failure needs no new code

Task #75, stage s95.

- **Reading taken:** the seven failures carry the prefix "the agent failed:", which only a `Crash` produces. Since s83 a lost agent host is `FailReason::HostLost`, restarted up to `MAX_AGENT_RESTARTS` times by `settle_all`, so that signature cannot recur on this code.
- **Not changed:** the restart limit, the retry ladder, and the detector. A repeat of the new text ("lost N times") is a different signature and should be investigated from `step_host_losses`.
- **Open:** whether the descriptor-limit fix (5110d9b) removes the underlying losses is not proven here.
