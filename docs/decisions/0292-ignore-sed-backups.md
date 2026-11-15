# 0292: ignore sed `-e` backups

**Decision.** Delete `introspect.rs-e` and ignore `*.rs-e`, `*.swift-e`, `*.kt-e`, `*.toml-e`, `*.md-e`, `*.sh-e`, `*.py-e`.

**Why.** BSD `sed -i -e` writes a backup with suffix `-e`; one was committed and showed up in searches.

**Trust.** `.gitignore` is not protected; no gate is changed.
