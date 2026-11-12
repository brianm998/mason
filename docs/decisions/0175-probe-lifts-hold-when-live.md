# 0175: A probe lifts the hold when the provider accepts it, not when it ends

A hold on an account was cleared only when the probe's whole session ended. After a login, task #190 ran fine as the probe but the hold stayed for the rest of its session and three tasks kept waiting.

**Decision.** The first event that proves the provider accepted the session (output tokens, text, a tool call) clears the hold via `holds::probe_ok`. `Model` and `Usage` do not: a refused session can emit them.

**Kept.** A session accepted and refused later still puts the hold back through `observe_end`; manual and budget holds are never cleared by a probe.

**Text.** Logging in does not lift a credentials hold; the next good probe or `mason account release` does. The hold text and alert now say so.
