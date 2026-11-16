# 0443: exit scripts that run Swift or Gradle must test the skip variable

CI's Linux `exit-scripts` shards set MASON_SKIP_SWIFT and MASON_SKIP_COMPOSE because the monitor clients have their own jobs. Scripts written from known-red tasks had no guard and failed every run.

**Decision.** `scripts/check-exit-guards.sh` fails when an exit script runs `swift test|build` without MASON_SKIP_SWIFT, or `gradle` without MASON_SKIP_COMPOSE (`-real`/`-manual` scripts are never run by CI and are exempt). Fix tasks filed by the known-red ledger (decision 0225) state the guard requirement in their body when the suite is a client. The ledger's task text is not verdict logic.

**Not verified:** the CI Linux run.
