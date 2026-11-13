# 0186: One version, one asset naming contract

**Decision.** The workspace version in `Cargo.toml` (0.1.0) is the single source: binaries report it, `release-version.sh` checks a tag against it, the Swift bundle and Compose build read it. Release assets are named only as in `docs/RELEASING.md`, which every packaging task and the download page follow.

**Why.** Nothing tied a tag to what binaries report, and later release jobs need agreed names. `--version` carries the commit so a build is traceable; it is `unknown` when neither `MASON_BUILD_SHA` nor git is available.

**Trust.** No file in `PROTECTED` changed.
