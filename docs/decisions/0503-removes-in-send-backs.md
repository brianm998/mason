# 0503: dropped-line send-backs carry ready-to-paste removes entries

**Decision.** For each listed dropped line that the task's diff replaced in place, the send-back includes a `{"file","line","reason":"<fill in>"}` entry and says: paste it into `removes` if the replacement was on purpose, otherwise restore the line.

**Why.** #363 was sent back three rounds because the agent did neither.

**Trust.** `crates/landing/**` is in `PROTECTED` (verdict logic). The verdict is unchanged; only the message text grows. Needs the owner's approval.
