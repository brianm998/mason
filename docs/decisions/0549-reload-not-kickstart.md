# 0549: sessions apply config edits with reload, not a kickstart

## Context

Decision 0152 added `mason project reload`, yet the log still shows `masond stopping: SIGTERM from pid 1` minutes after a successful live reload (2026-11-16 16:50:58 reload, 16:55:27 SIGTERM). The sender is launchd, so a session ran `launchctl kickstart -k` anyway. Each restart re-queues in-flight test steps.

## Decision

- Sessions apply a registration edit with `mason project reload --project P` and do not kickstart afterwards.
- A restart is for a changed `path`, `base`, `remote` or `[self]`, a new registration file, or a new build, and those are batched.
- The daemon does not try to block launchd; the rule is stated in CLAUDE.md and docs/self-hosting.md.

## Consequences

No daemon code changes. The rule depends on sessions reading the docs. Restarts with no preceding reload (16:55 and 18:23 on 2026-11-16) are not explained by this and are not addressed.
