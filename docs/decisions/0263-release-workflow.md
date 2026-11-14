# 0263: the release workflow drops unproven assets and fails on a missing one

**Decision.** `.github/workflows/release.yml` attaches an asset to the draft release only if the end-to-end job that installed it succeeded, creates the draft before checking completeness, and then fails if any asset in docs/RELEASING.md is missing. Unsigned macOS builds are removed at publish.

**Why.** Star shipped a broken Windows installer because nothing checked its install job. A failed final step leaves the draft for inspection but makes the run red.

**Trust.** `.github/workflows/**` is PROTECTED; this change waits for the owner's approval. ci.yml is untouched. Nothing weakened; `permissions` is read-only except `publish`.
