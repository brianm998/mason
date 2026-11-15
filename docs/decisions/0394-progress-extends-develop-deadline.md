# 0394: a develop step's deadline grows while the agent makes progress, up to twice the budget

**Decision.** A develop agent that commits or produces output gets budget/4 more time each time its deadline is within 2 minutes, up to one extra budget in total. Queue credit (s363) is separate and uncapped. A step that times out says how its time divided.

**Why.** 22 develop steps ran out of their 4-hour budget in four days. Whether queue time was the cause after s363 is not yet known (see s401); an agent that is still committing at the budget is better served by more time than by losing the work, and the cap keeps a loop from running forever. The budget default itself is unchanged.

**Trust.** Budgets are an owner decision. The default budget is untouched; only the extension rule is new. It needs the owner's approval: the cap (1x the budget) and the grant size (a quarter) are my choices.
