# 0395: a wait's class is a function of its cause

Decision: `mason_scheduler::cause_class` is the one mapping from a wait cause to a class, and it never returns an empty string. The daemon's fallback names `other` instead of an empty class.

Why: empty classes show as "?" in the monitor and in totals. Existing class names stay, so old and new totals add up. No backfill.

Not verified: the live log's classless waits were not inspected.
