# 0738: send the build in its own field

Status: accepted

A suite's `build` and `command` are written for separate processes. Joining them with `&&` made two leading `cd`s fail on the runner. Both remote paths now put the build in `RunSpec.build`; every runner path supported by `mason-run.yml` takes it, so no `(b) && (c)` fallback was needed. `suites.rs` is a protected file; the change only moves the build into the field made for it and does not change how a run is judged.
