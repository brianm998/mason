# 0975: Add Project is three owner-only daemon endpoints over mason-onboard

Stage s982, task #885. The repository to add is on the daemon's machine, so the daemon does the
work and a monitor only collects answers.

Decision:

1. `GET /onboarding/candidates`, `POST /onboarding/plan` and `POST /onboarding/apply` each call
   `owner_only` first. Roots come from `[onboarding] roots` in `daemon.toml` (default `["~/git"]`).
   Add Project is offered on the desktop monitors only (owner decision 2026-11-21).
2. `apply` is one synchronous call that returns its progress lines, as the editor's apply returns
   its result; no job id to poll. It refuses before writing anything for a path inside a registered
   project, a taken id, a missing repository and (for `land = merge`) a dirty tree.
3. **Landing from the monitor follows decision 0290**: only the owner's explicit answer to the
   landing question (`no`, `merge`, `push`, `pr`) lands anything; the default is `no`. The endpoints
   call `mason-onboard`, which calls `mason-landing`; neither is changed.
4. The proof task is on by default and skipped by `no_prove`, as in the CLI. It is filed even when
   the setup branch was not landed (the CLI waits for that); the response says what happened.
5. The `mason init` CLI keeps calling `POST /projects/{name}/register` and is not routed through
   `/onboarding/apply`.

Not verified: `push` and `pr` through HTTP; a real monitor against these endpoints.
