# 0809: One placement mode at three levels

Stage s816. Where runs go was set in three places that did not line up: the global `default` in
`remote.toml`, a task's `run_on` (`github`/`local`), and a project's `[remote_runs] fallback`. There
was no per-project choice and no "prefer one, spill to the other".

Decision: one mode, `local`, `remote`, `favor-local`, `favor-remote`, set globally, per project and
per task; the most specific set level wins. A project with no `[remote_runs]` is always local. Old
values keep reading: `default = "github"` is `remote`, `fallback = "local"` is `favor-remote`
(`"none"` is `remote`), `run_on = github` is `remote`. The suite's own rules (`remote = false`, a
device, `exclusive`, a failed setup) still keep a run here under `remote` and both favor modes. Both
favor modes go through `place::balance`; until the load rules of the follow-up tickets land,
`favor-local` is `local` and `favor-remote` is `remote` with the s812 bounded fallback.

The owner asked for this on 2026-11-21. No protected file is touched.
