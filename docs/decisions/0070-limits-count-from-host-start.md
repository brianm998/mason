# 0070: Time limits count from when the host started

A step's time limit is measured from `started_ms` in the host's `host.kv`, which is written once and survives the daemon, not from the moment the daemon attached. One helper (`registry::remaining_limit`) serves the agent attach and both test-run watchers. A limit already passed at re-attach stops the run at once and it ends as a timeout.

A new invocation (nudge, answer, resume after a hold) is a new host and has its own limit, as before; a budget spanning invocations is a separate question. No gate, budget value or verdict rule changed; `suites.rs` (verdict logic) only changes where its clock starts.
