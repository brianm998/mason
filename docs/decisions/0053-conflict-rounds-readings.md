# 0053: Readings made while taking conflicts out of the round count (s68)

- **A conflict is not a weaker gate.** A conflict send-back never produces a pass. The task goes back to develop and must be green on a complete run against the base before it lands, exactly as before. Only the count of red rounds changes, and a change that is red still fails after `max_rounds`.
- **A separate limit, not none.** Five conflict send-backs, then the task fails with what kept conflicting. An agent that cannot keep up with the base, or a conflict it cannot resolve, still ends.
- **Counted from what the world records, plus one file.** A round is worked out from the task's step runs (the events), and the conflict runs come from `conflicts.txt` in the task directory. Adding an event kind for it would have changed the event log's schema for a counter.
- **A retry starts the rounds again.** The owner retrying a failed task is a decision to try again. Charging it the earlier rounds made a retry one red away from failing.
- **Numbers:** 68 and 0053 were the next free, and were reserved in the store under task 900068 before the files were written.
