# 0441: monitor_inplace.rs uses the platform process helper, not std::os::unix

The in-place monitor build starts and stops its process tree through `mason_platform::process::spawn_group` and `signal_group`, which already hold the Unix and Windows variants, so the file has no platform code of its own. Its tests need executable shell scripts and are `#[cfg(unix)]`. Rejected: a second `cfg(windows)` block with its own `CREATE_NEW_PROCESS_GROUP` (duplicates the helper). A Windows `cargo check` in the landing batch check is left for the owner, as it changes a gate.
