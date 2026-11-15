# 0388: the review step settles base lines a task replaced on purpose

**Decision.** When the landing audit finds a base line missing from the task's tree and the task's merge changed the same place, the line is paired with its replacement and shown to the review step as "replaced on purpose?". The reviewer's approval declares the pairs as `removes` with a reason; a rejection sends the task back naming them. A line dropped with no replacement nearby is sent back as before. This revises decision 0117.

**Why.** A rewritten line and a line lost in a bad merge looked the same, so deliberate rewrites cost send-backs each round (star #216). The call belongs to a reader of the task's purpose, not only to the author's `removes` list.

**Trust.** `crates/landing/**` is in `PROTECTED`, and the review submission contract in `crates/adapter-api` changes too. No check is loosened: the audit still flags every undeclared line, and only a reviewer's declared reason clears a paired one. `crates/daemon/src/prompts.rs` is not changed; the instructions travel in the block `review_block` appends to the review prompt.
