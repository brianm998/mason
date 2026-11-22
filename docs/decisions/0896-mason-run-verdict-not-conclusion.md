# 0896: the verdict is the artifact, not the run's conclusion

About 700 failure emails a day came from `mason-run` runs that were red only because the final step exited with the suite's code. MASON already takes the verdict from `result.json`, so that step now exits 0 whenever a result exists and fails only when there is none (setup, upload or runner loss). A run cut off by the timeout has no result and stays failed; MASON already treats a missing artifact as environmental. Sharding is unchanged (fail-fast false). The workflow is a protected path and needs owner approval.
