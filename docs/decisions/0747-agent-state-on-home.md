# 0747: home shows agent state and runs on GitHub

Status: accepted (#763)

`active` on home is every agent with a state: `working` when it holds its `llm` slot, `waiting_check` (`github` with the run's link, or `here`) when its `run_check` is out, `waiting_slot` when a started step waits for a session. A queued step that has not started stays out, as before. `remote` counts the runner units of runs in flight by pool and by kind. The kind and task are written into each remote hold, the file the daemon already keeps for runs that are out, so no new polling or GitHub call is made. No gate, limit or verdict logic changed.
