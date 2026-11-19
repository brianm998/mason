# 0622: gradle exit scripts skip under MASON_SKIP_COMPOSE

Status: accepted (s629).

Context: shard 5 on ubuntu failed s299 (s469-real unlisted, no RESULT line) and s450 (guards flagged s451 and s572 for running gradle without MASON_SKIP_COMPOSE).

Decision: s451 and s572 skip gradle under MASON_SKIP_COMPOSE, as s433 does. The sample's unit tests move to the device-sample-android CI job (`:app:testDebugUnitTest`).

Trust: scripts/exit/** and .github/workflows/ci.yml are protected and need owner approval.
