# 1095: a content scan for secrets and banned words

`contentscan` sits beside `markers` and shares its reading of added lines. Choices the task did not settle:

- **Every match is masked** in the quoted line, secrets included. The task asks that a hit name the line cut short and never the list; a quoted banned word is a piece of the list, and a quoted secret would be copied into feedback and logs.
- **`mason:allow-secret` allows secrets only.** It must be a trailing comment. A banned word is the owner's rule, and an agent must not be able to waive it.
- **The generic assignment** needs a quoted value of 16 or more characters, without spaces, with a letter and a digit, that is not a reference (`env:`, `file:`, `keychain:`) or a placeholder (`${…}`, `<…>`, `{…}`). This misses some real passwords in exchange for few false hits.
- **A broken list stops landing, not development.** An unreadable file or unusable expression fails the landing job, naming file and line. `submit_change` and `run_check` apply the entries that do work: an agent cannot fix the owner's file.
- **The batch log** is `<data>/batches/<id>/content-scan.log`, also written to the daemon's log.
- **`run_check` measures from the merge base**, so a line the base has since removed is not blamed on the change.
- **`masond contentscan`** is the scan by hand, for the exit script.

Not covered: commit messages, task bodies, file names, binary files, untracked files at `run_check`, text added by one commit and removed by a later one (the refusal asks the agent to rewrite history, but nothing checks it), and edits a landing resolver makes while folding. No CI secret scan was added.
