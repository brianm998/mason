# 0328: top bar mascot follows the busiest active state

**Decision.** The top bar shows the illustration of the active state with the most tasks in the header counts. Landed and canceled are excluded; a tie goes to the later stage; with no tasks it shows the idle art (`all_clear`); disconnected it shows nothing.

**Why.** The counts are the tiles' own, so the picture follows the project filter. Click reveals the sidebar section, since state detail pages have not landed.

**Trust.** `clients/features.toml` is in `PROTECTED` ("gates: client parity"); the new row waits for the owner's approval. No gate is changed and `clients/mascot.toml` is untouched.
