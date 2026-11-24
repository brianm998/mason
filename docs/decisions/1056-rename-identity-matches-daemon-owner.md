# 1056: rename identity matches the daemon's owner

The CLI takes the owner identity for `project rename` as MASON_AS, else USER, else LOGNAME, else "owner": the daemon's default owner list order (config.rs). Using only USER failed on CI where it is unset.
