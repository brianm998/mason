# 0755: do not guess a fix for the Linux exit-scripts red

Decision: with only the log tail, the failing assertion in s80/s449 is unknown, so this change records that and names the command that settles it rather than editing tests blind. Rejected: loosening the `holds.rs` time ranges or lengthening sleeps (guesses, and they weaken the tests).
