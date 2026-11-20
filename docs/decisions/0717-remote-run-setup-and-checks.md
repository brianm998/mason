# 0717: how remote runs are set up and checked

Context: remote runs (#697) need a workflow on the base branch, a token, no second workflow firing on `mason/run/**`, and push protection, and crashes leave run refs behind.

Decision: `mason doctor` checks all four for each project with `[remote]`; `mason remote setup` writes the workflow on a branch (nothing pushed, as `mason init` does for `mason-ci.yml`); MASON gets its own `mason-run.yml`; the existing safety sweep deletes `mason/run/**` refs over 24 h old with no live run record. Workflows are scanned as text, like the landing-CI check, because the workspace has no YAML parser and `deny.toml` should not grow for it; the scan errs towards "fires", so a workflow it cannot read is reported. Push protection off is a warning, not a failure: it is the owner's account setting, and a run still works. The sweep asks GitHub for each ref's commit time instead of fetching the refs, so it adds no git objects to the project's repository; an unknown age keeps the ref.

Consequences: a token's own scope is not probed (only the user's `permissions.push`), because the only exact test is to push a ref. The sweep needs the token to read commits. `.github/workflows/mason-run.yml` is a trust file (`protect.rs`); it adds one workflow and changes none, so it needs the owner's approval.
