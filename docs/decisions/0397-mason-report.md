# 0397: `mason report` is a separate read of the log, not an extension of the digest

Decision: the report has its own module and endpoint, `/report`, and does not add to `digest`.

Why: the digest replays the log through `Replayer` for time per state; the report needs per-wait lengths (for a median), per-day counts and normalized reasons, none of which the digest's `Replay` keeps. Adding them would change the digest's cost and shape for every caller. The report reads the log once and the store tables once.

Trust: none. No gate, guard, budget or verdict changes.
