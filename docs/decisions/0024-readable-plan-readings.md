# 0024: Readable plan, the choices the ticket left open

Task #16. Each is the reading taken; change one and the others stand.

- **Parsing lives in `crates/daemon/src/plan_view.rs`**, a pure function `parse_plan(&str) -> PlanView`; `views.rs` only calls it. `PlanView` is set only when `plan.json` exists; the old `plan` string stays filled.
- **Text before the first list marker of the approach is a `lead`**, an unnumbered paragraph above the steps, not a step.
- **`steps_are_list` and `step_numbers`.** The numbers shown are the agent's own ("0", "1", "1)" gives "1"), one per step, so a plan numbered from 0 shows 0. A bulleted list has none (clients draw a bullet). Not a list: the paragraphs go in `steps` and `steps_are_list` is false. A mix of numbers and bullets is a bulleted list.
- **Markers:** `\d+[.)]` or `-` / `*`, each followed by whitespace. Continuation lines are trimmed and kept with their step, joined by `\n`.
- **Other values:** a string as is; a list of strings as `- ` lines; anything else as indented `key: value` lines, nested by two spaces, no braces or quotes. Null is empty.
- **Files and criteria** keep the agent's order; non-string items are rendered readably, empty ones dropped.
- **Expand rule:** steps, files and criteria are open when the plan has fewer than 10 steps, closed with their counts otherwise (`PlanText.expandedByDefault`). "Other" is always open.
- **A daemon without `plan_view`** (an older one) shows the plan text wrapped in body font, no note.
- **Protocol version stays 1**: additive, `TaskDetailView.plan_view = 19`.
- **Gate files touched, for the owner's approval:** `clients/features.toml` (protected: client parity) gets `[feature.readable-plan]`; `scripts/exit/s46.sh` (protected: the exit tests) is new. No test, limit or gate is weakened.
