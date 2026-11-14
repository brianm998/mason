# 0287: parallel and conditional steps arrive in phases; gate first

**Decision.** #318 is too large for one change. This change makes `gate` steps runnable and leaves fan-out, conditions, `ci`/`pr`, post-land steps and editor controls refused by validation until each lands.

**Why.** Fan-out changes the task's single current step, the event log and the store codec, and post-land steps change `landing.rs`. Both need the owner's approval on event shape and protected files. A gate is just a check command, so it can run now without those.

**Trust.** `crates/definition/src/validate.rs` (validation rules, `Capabilities::now()`) only widens what is accepted for `gate`; no check was weakened. The two tests edited keep their intent (a step type that cannot run yet is refused) and now use `ci`. `scripts/exit/s293.sh` is a new exit script. Nothing else protected is touched.
