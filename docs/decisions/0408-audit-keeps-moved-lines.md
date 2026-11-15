# 0408: the dropped-line audit keeps a line the task moved to another file

**Decision.** A line the base landed, missing from its own file after the task's merge, still counts as kept when it is, trimmed, a whole line of another file the task changed beyond that base commit. This revises decision 0117, which looked only in the line's own file.

**Why.** A task that splits or moves code must move the base's new lines too. Under 0117 the audit called every such line dropped, so a split could never land while others kept landing in the file it split (star #417, 2026-11-14). Listing moved lines under `removes` would say "removed on purpose", which is false, and would hide a later real drop of the same line.

**Trust.** `crates/landing/**` and `scripts/exit/s415.sh` are in `PROTECTED`. The owner approved this on 2026-11-15 ("Yes, fix and retry"). The match is stricter than the one in the line's own file: exact rather than by token window, and only in files the task wrote.
