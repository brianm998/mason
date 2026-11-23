# 1023: Land now holds only while the freeze is in force

Decision: the test asserts that Land now does not pass a freeze while it holds, and that the task lands once the freeze lifts. No daemon change.

Alternative left to the owner: Land now lapsing when a freeze lifts, so a pressed task would not land at once.
