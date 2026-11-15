# 0391: hold a queued task while an in-flight task changes the same files

**Decision.** A queued task whose plan or explored body names a file that a task in testing, review or landing changes waits until that task lands, at most `hold_back_secs` (default 7200), and never for `hold_back_ignore` globs.

**Why.** 39 send-backs in four days were base conflicts, concentrated on a few hot files. Waiting costs less than a second merge. The time cap keeps a stuck task from holding the queue; the ignore list keeps always-changing files from serialising everything.

**Trust.** None: no gate, guard, budget or rank code changed.
