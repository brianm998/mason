# 0633: exit gates use medians and condition waits

Decision: an exit gate on latency gates on the median of N=10 runs against the unchanged target (250 ms, 100 ms), and a gate that waits for the daemon waits on a condition with a timeout, not a fixed sleep. A fixed sleep stays only where the time is the rule under test (the 30 s takeover) or the check asserts nothing happened.

Why: single wall-clock samples fail on a shared CI runner without any change in design. A median still fails if the design is slow. Tests are selected by exact name so a substring filter cannot pull in a timing-sensitive test.
