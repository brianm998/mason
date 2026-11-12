# 0152: reload registration files in place; a stop is not a crash

**Context.** On 2026-11-11 five `launchctl kickstart -k` restarts, made to apply edits to `~/.config/mason/projects/*.toml`, looked like crashes: SIGTERM killed masond silently, and a start with no `healthy` after it counts toward the three-in-five-minutes rollback.

**Decision.**
- Handle SIGTERM/SIGINT: log, graceful bounded shutdown, `stopped` in boots.log, exit 0. `crashes_in` treats `stopped` as an ending (a change in PROTECTED `update.rs`, needed because otherwise a stop before `healthy` is still counted).
- Add `mason project reload`, a live reload of the registration file. Fields that other state is built on (`path`, `base`, `remote`, `[self]`) are refused rather than half-applied; the owner restarts for those.
- The registry holds leaked `&'static ProjectCfg` so the many `&ProjectCfg` borrows (some held across long jobs) stay valid when an entry is replaced. The alternative, `Arc<ProjectCfg>` everywhere, touched over a hundred call sites for no gain at a few KB per reload.

**Consequences.** The sender's pid is not logged. Docs now point to the reload command instead of `kickstart -k`.
