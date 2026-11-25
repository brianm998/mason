# 1092: the dedupe test compares usage content, not silence

## Context

The server skips a view whose encoding hashes the same as the last one sent of that kind (`daemon/src/ws.rs`). The usage view includes clock-relative text, so "no event touched usage" does not mean "usage is unchanged". The test assumed it did and failed on CI.

## Decision

The test asserts what the server promises: no two consecutive usage views it sends are equal. A broken dedupe still fails it whenever the clock text has not moved, which is nearly every run.

## Rejected

- Injecting a fixed clock into the daemon: a product change for the sake of one test.
- Seeding the demo with offsets away from minute boundaries, or sleeping past one: narrows the race without removing it.
- Ignoring the clock fields when comparing, or when hashing in the server: monitors show those strings, so a change in them has to be sent.
