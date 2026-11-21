# 0728: Remote-run settings live in `[remote_runs]`

`[remote]` clashed with the top-level git remote key `remote = "origin"`; TOML allows one or the other, so no registered project could enable remote runs (#744).

**Chosen.** The table is `[remote_runs]`, same keys and `[ci]` defaults. A project file with a `[remote]` table fails to load with a message naming the fix. A suite's own `remote = false` is unchanged.

**Not chosen.** Renaming the git remote key would have broken all five registration files and the daemon's config reload.

**Not verified.** The owner's real `mason.toml` was not edited or reloaded here.
