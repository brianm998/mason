# 0973: the displayed expected total comes from base-level runs only

Chose a separate `suite_size` table over stopping task runs writing `suite_green`, because gates read `suite_green` and its task-written rows must keep deciding what they decide. Cost: until a base-level run (landing batch, scheduled full test) records a size, a test step has no expected total (or its declared minimum) instead of a possibly wrong one. A live count past an expected/minimum total raises the total rather than dropping it.
