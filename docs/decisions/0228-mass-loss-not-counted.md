# 0228: losses shared with other tasks do not count against an agent step's restarts

**Context.** Four tasks failed after three lost hosts each, all signal unknown.

**Decision.** A lost host whose last heartbeat is within 30 s of another run's lost host does not count toward MAX_AGENT_RESTARTS; four times that still ends a run.
