# 0838: flaky daemon test races

Status: hypotheses, not reproduced locally (`cargo test` is refused in the task shell; the GitHub
failure messages were not available). Confirm on the pipeline's runs.

- `agentshim::tests::the_off_switch_lets_a_test_run_through` (agentshim.rs, `world()`/`run()`):
  fake tools were written and exec'd in a multithreaded test binary; a fork from another thread can
  hold a writable fd copy, so exec fails with ETXTBSY / exit 126. Fix: write temp then rename, and
  retry "Text file busy" in `run()`.
- `crash::kill_9_in_the_middle_of_an_agent_session_loses_nothing` (crash.rs): the second kill came
  right after `wait_daemon`, before recovery. Fix: wait for the task developing and a running host.
- `targets::a_skipped_suite_still_tears_its_device_down` (targets.rs): 30 s teardown wait is short
  under load; the stop runs on a 100 ms poll thread. Fix: wait budget 120 s.

First reproduced failure messages: none yet. The 20-runs-on-GitHub criterion is unverified.
