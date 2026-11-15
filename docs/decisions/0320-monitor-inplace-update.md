# 0320: a bare `swift build` monitor is rebuilt in place, by the daemon

**Decision.** The monitor sends its executable path and commit with the update request. If it is a bare binary in a git checkout of this repository's `clients/swift/.build`, the daemon builds the channel's newest monitor commit in its own build directory (not the checkout), under a `compile` lease, and replaces the binary at that path by rename after success. The monitor starts that path again and quits. Anything else (the `.app`) follows the existing bundle path.

**Why.** The owner runs the bare binary and keeps that setup. It reported `dev`, which never nags, and the bundle path cannot replace a bare binary. The daemon already owns fetching, the mirror, compile slots and the header, so the build lives there and the monitor only shows progress.

**Stamp.** A generated `BuildStamp.swift` (checked in empty) carries the commit into any build made by the daemon or `make`. A hand-run `swift build` stays `dev` and quiet.

**Never downgrade.** The daemon refuses a commit that is an ancestor of the monitor's own, and the same commit.

**Trust.** `crates/daemon/src/update.rs` is in `PROTECTED`; the change there is only the monitor state fields, their copy into the header and three visibility widenings, and needs the owner's approval. The build, checks and install are in `monitor_inplace.rs`.
