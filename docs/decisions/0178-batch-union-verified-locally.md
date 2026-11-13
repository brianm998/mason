# 0178: the batch union is verified locally before CI

Status: accepted (stage 183, task 206)

## Decision
A project in a CI or PR landing mode declares a `batch` tier, so `verify()` runs cheap suites over the union diff before anything is pushed. star's example gets `t0-pipeline-static`, `t0-diff-check`, `t0-i18n`; builds, tests, `t0-proto-sync` and `t0-gherkin-generated` stay in CI. `mason doctor` warns when the tier is missing. No verify, ladder or PROTECTED code changed; no gate weakened.

## Owner action
MASON cannot edit star's repository. Copy the `[[tier]] name = "batch"` block from `docs/migrations/star/.mason/landing.toml` into star's live `.mason/landing.toml` (or re-run the star migration). Until then `mason doctor` warns.

## Cost
If a tier red reaches `rerun-failed`, a deterministic lint is rerun once before blame-by-impact; accepted.

## Not verified
The live star file; a real `uv run ruff`/`pyright` run on a star union (the test's suite is a stand-in duplicate-line check).
