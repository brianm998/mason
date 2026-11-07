# 0030: Readings made while changing the self-hosting step cap (s52)

This changes `crates/daemon/src/selfguard.rs`, a trust file; it loosens a guard and waits for the owner's approval.

- **Rule.** `max_steps` counts only agent steps of tasks whose trust is not `Owner` (`OwnerAgent`, `Outside`), by the one predicate `is_owner_filed`, which `count` (per_week, decision 0022) also uses. Owner steps are shown as "yours not counted".
- **Why it is safe.** The cap bounds spend in a loop where MASON files work for itself; a person filing their own tasks is not that loop. An owner identity (and any client that reaches `POST /tasks`) can now spend without a step cap. Guards that remain: `max_open`, the agent-filed share of `max_steps`, `per_week`, the cooldown, and the holds on protected files.
- **Where it bites.** The cap is enforced at filing and import only; nothing stops a running step. "Stall" means agent and outside tasks are refused or not imported at the cap, with the count, the limit and why owner steps are excluded.
- **Changed test.** The `selfguard.rs` limits test asserted the owner is refused at the step cap; it now asserts the owner passes and OwnerAgent/Outside are refused. No other test changed.
- **Not verified.** Real GitHub; monitors (nothing reads `/self`); the exit test uses command steps in the developing category rather than fake-claude agent steps, since the cap counts step runs by category.
