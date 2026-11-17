# 0517: MASON watches its own CI on develop, without the landing freeze

`.mason/mason.toml` now has `[watch.ci]` (decision 0149): `enabled = true`, `gate = "CI"`, `freeze_on_gate = false`, `max_open_auto = 2`. Before, only star's CI was watched, and develop's reds (Windows build, Android sample, fourteen exit scripts on 2026-11-15) were found by hand.

**Gate.** `gate` takes one value, `"workflow"` or `"workflow/job"`, not a list. The wanted jobs (`lint`, `test`, `windows`) are all in the `CI` workflow, so `gate = "CI"` gates every job in it, including `dco`. A red there files at once, urgent. The other workflows (Android, iOS, ...) are watched with the default `agreement` of 2. Narrowing to single jobs would need a list form in `watch.rs`; not done here.

**No freeze.** MASON lands `direct` and fixes its own CI through the pipeline. A freeze on a red gate lets only the fix task land, and would stop the tasks that repair it. To turn it on later: set `freeze_on_gate = true` in `[watch.ci]` (a reviewed change to a protected file).

**`max_open_auto = 2`**, as for star: more reds than that become proposals.

**The owner must add `[ci]`.** The watch takes the repository and token from the machine's project file, `~/.config/mason/projects/mason.toml`, which has no `[ci]` section. Add a `[ci]` section with `kind = "github-actions"` and `repo = "brianm998/mason"`, plus the token key that star's `[ci]` section uses. Without it `mason status` says the watch cannot look, and nothing is filed. This change does not edit that file.

**Not verified:** a live pass against GitHub; the `[ci]` keys are taken from decision 0149's description, not from a working example here.
