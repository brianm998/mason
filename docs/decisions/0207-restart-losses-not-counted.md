# 0207: A host lost at a daemon restart does not count against the task

Four launchd restarts on 2026-11-12 lost every running host, and five tasks failed with "the step host was lost 3 times" though each loss was a separate restart.

**Decision.** A lost host whose last heartbeat falls within the 120 s before a recorded daemon start is a restart loss: recorded, and the step still restarted, but not counted toward `MAX_AGENT_RESTARTS`. A loss caused by the daemon going away is not the task's failure. The launch-agent example sets `AbandonProcessGroup`.

**Trust.** Changes a restart budget (hostloss.rs, jobs.rs, neither protected); it only excludes losses the daemon caused, and a real loss still counts and still fails the step on the third. Landing (protected) is untouched; adopting batches waits for the owner.

**Unverified.** That `AbandonProcessGroup` stops the deaths: a test launch agent did not reproduce them with or without it.
