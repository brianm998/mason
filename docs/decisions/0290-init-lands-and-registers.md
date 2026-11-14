# 0290: `mason init` lands its setup on request and registers the project with the running daemon

Task #322. Decision 0057 left the setup on an unmerged branch ("`mason/setup` is never merged by `init`"), made `init --prove-only` a second command and left the monitor unopened. This supersedes those readings, and the three-command text that followed from them, only.

1. **Landing is the owner's explicit answer.** `--land no|merge|push|pr`, asked on a terminal, default `no`. `--yes` and a missing terminal both mean `no`, so `init` still never merges, pushes or opens a PR on its own. `merge` is a fast-forward and only when the base is checked out and tracked files are clean; otherwise it refuses, prints the commands, and changes nothing. In `pr` mode `push` sends the setup branch, not the base. The PR uses `GITHUB_TOKEN` or prints the command.
2. **A new project needs no restart.** The earlier text assumed registration reload (#172) removed it. It does not: `reload_registration` refuses a name masond did not start with. A new `POST /projects/{name}/register` appends `ProjectRegistered`, inserts the project and reads its definition. This is an addition to the daemon's registry code, not to a gate, guard, budget or verdict.
3. **The monitor** is opened only for someone sitting at a Mac terminal with the app installed; elsewhere `init` prints a pairing code (or `mason pair`). Nothing automated is put on a screen.
4. **A summary** of where definition, landing, alerts and providers live closes the run.

Not decided here: the ten-minute target is still unmeasured on a real machine.
