# 0394: a develop step's deadline grows while the agent makes progress, up to twice the budget

**Decision.** A develop agent that commits or produces output gets budget/4 more time each time its deadline is within 2 minutes, up to one extra budget in total. Queue credit (s363) is separate and uncapped. A step that times out says how its time divided.

**Why.** 22 develop steps ran out of their 4-hour budget in four days. The store shows 60 timeouts after s363, all within an hour of a daemon restart: the restart dropped the in-memory queue credit while the host kept counting from its start. The credit is now kept on disk and restored (see s401); an agent that is still committing at the budget is better served by more time than by losing the work, and the cap keeps a loop from running forever. The budget default itself is unchanged.

**Also.** Credit and grants are persisted per run so a restart cannot cancel them; no default changes for this.

**Trust.** Budgets are an owner decision. The default budget is untouched; only the extension rule is new. It needs the owner's approval: the cap (1x the budget) and the grant size (a quarter) are my choices.
