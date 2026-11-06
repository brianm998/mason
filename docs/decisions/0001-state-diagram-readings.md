# 0001: Readings of Part 4's diagram

Stage 2 followed the diagram literally where it was explicit and chose the
following where it was silent. Recorded so the owner can overrule any of them.

- **Answered goes to `developing`**, as drawn, even if the question came from
  `exploring`. The agent resumes in `developing` with the answer.
- **Review cannot ask.** The diagram's needs-attention arrows start at
  exploring, developing, testing and landing only.
- **Cancel** works from proposed, queued, every step state and needs
  attention. It is not offered from `failed` (the diagram omits it; retry is
  the way out) or from landed/canceled.
- **Closed with no change** is allowed from queued, every step state and
  needs attention, and releases blockers like a merge does.
- **Retry** is allowed from failed, needs attention and every step state
  (Part 10's "retry from step"), to any of the five step states.
- **Activity** adds `Idle` for states with nothing to do (proposed, failed,
  canceled, landed). Needs attention is `Waiting(Owner)`.
- **A canceled or failed blocker does not release** a held task; it stays
  held until the owner acts.
- **Ids are allocated by the reducer** from counters in the world, so replay
  is deterministic; the new task id is reported as `Effect::TaskCreated`.
