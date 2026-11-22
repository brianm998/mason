# 0810: debts rank in their own band behind all task work

Status: accepted (owner asked for it on 2026-11-21; touches `crates/scheduler/**`, protected).

A debt is owed work on already-landed code. It ranked as its task's test step, and a fallback run ranked by its task's current step, which for a landed task is the landing band. Both put debts ahead of live work.

Decision: `Rank::debt()` sorts after every other band, ordered by age then id, and is used by `hold_for_debt` (slots and runner units) and `fallback_lease` for any task not in a step. Only the order of grants changes; no limit, gate or verdict does. A held debt is never stopped.
