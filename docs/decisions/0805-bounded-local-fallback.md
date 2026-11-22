# 0805: A local fallback from GitHub takes this machine's slots

Stage s812. With `fallback = "local"`, a run GitHub could not start ran here at once: a run bound for
GitHub skips the local slot queue (#759, #761), and nothing asked for the slots again when it came
back. Many could start together, past the `compile`/`test-suite` limits and the CPU and thermal holds,
the kind of storm that overheated this machine on 2026-11-19.

Decision: on fallback, the run asks the broker for the slots it holds when placed here (a check's
`holds`; a test step's or debt's test-step `uses`) before it starts, ranked as its task's work. The
broker's limits and holds then decide when it runs. The owner asked for this on 2026-11-21 and for it
to be made outside the pipeline.
