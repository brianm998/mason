# 0468: the daemon's integration tests are merged into one executable

Status: accepted (no trust file changed; `.mason/**` was read only)

Each file in `crates/daemon/tests` was its own executable that linked the whole daemon, 89 links per build. 81 of them are now modules of `tests/it/main.rs`. Eight stay separate: two set process-wide environment variables, one sets the process-wide git deadline, five are listed in `.mason/serial-tests.txt` because they fail under load. Test names gain a module prefix, and the exit scripts that named a target now name `it` and the module. Rejected: merging everything (the environment-variable files would change each other's tests), and keeping every file as a target behind a wrapper (no saving). The count of tests is unchanged (931).
