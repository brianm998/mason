# 1149: a node without the job's base is a node failure, repaired from the coordinator

## Context

A node's mirror has no remote, so only the coordinator can put `origin/<base>` in it. When that failed, nothing said so, and an agent asked the owner to fetch by hand in a checkout the owner could not find (trd #1187).

## Decision

- The node reports the outcome and the coordinator judges. A `failed` or `not sent` base ends the run whatever the command would have said, since a command that tolerates a missing ref can pass or fail wrongly.
- It uses the paths a node failure already has (`Fail::Environmental`). Only a nodes-only run differs: waiting silently would hide the cause, so it returns an environment error at once, which the test step and `run_check` already treat as an outage and retry.
- `NODE_PROTO` stays 1. The new status field and routes are additive. A node on an older runtime reports no base: its runs say `base: not reported (older node runtime)` and are judged as before, and `sync` and `show` name `mason node update`. A bump would stop every nodes-only project until each node was updated.
- The guidance for agents is in the failure text itself, which is what an agent reads. The prompt files (`prompts.rs`, `adapter-api/src/contract.rs`) are protected and unchanged.
- `sync` sends the whole history, with no exclusions, so it also repairs a mirror that lost objects. It deletes nothing, a lock file included.

## Consequences

A project whose base does not resolve on the coordinator can no longer take a node's result, even for a command that never reads it.
