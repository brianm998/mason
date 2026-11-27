# 1198: A skip is judged by the loaded pipeline

## Context

`suites::skip` judges a task that has not started (empty pinned def) by `d.current_def(project)`. `defs.rs` returns the built-in pipeline until the project's definition is in `by_project`. A skip arriving before that gave "there is no step `review`" instead of "only the test step can" (s1007 on CI).

## Decision

The skip handler in `http.rs` calls `reload_definition` for an unstarted task before judging it. A refused reload keeps the current pipeline, as before.

## Consequences

A skip of an unstarted task may fetch the remote. `suites.rs`, `defs.rs` and s1007 are unchanged.
