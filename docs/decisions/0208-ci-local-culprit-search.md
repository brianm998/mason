# 0208: a red that a mapped local suite reproduces is bisected locally

Status: accepted by the pipeline, awaiting the owner's approval (touches `landing.rs`, PROTECTED, and `.mason/**`).

**Decision.** `[ci.local]` in landing.toml names, per CI job, the local suites that cover it. A batch red only in mapped jobs: base red in the job too, then hold; mapped suites green on the whole union, then ask the owner ("environment difference"); otherwise bisect with the suites locally and re-verify the remaining union through CI once.

**Why.** Bisecting through CI cost 10-11 runs of 15-59 minutes for reds that star's `t0-pipeline-static` reproduces in seconds.

**Kept.** CI has the last word: the final union still goes through CI. An unmapped job, or a mapping to a suite the project does not define, bisects through CI as before. A red CI judged real still goes straight back to the tasks.

**Alternatives not taken.** Skipping the rerun rung for mapped reds loses nothing the local run does not answer; a flaky CI red shows as "green here" and goes to the owner rather than a rerun. Matching base jobs by workflow too would need the workflow in the failed list, which the command CI does not give.
