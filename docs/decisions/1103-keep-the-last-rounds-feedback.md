# 1103: keep the last round's feedback when a step fails

Feedback was written only when a task went back, so the round that failed it left nothing for the diagnosis or a retry (trd #1112: the diagnosis took the failure for a round cap on accepted code).

The save is now one helper used by every red end, whether it sends back or fails, including the owner's rejection. A failure that is not a red (an error, the conflict cap) writes nothing, so it cannot erase findings still waiting to be addressed. Because the failing round now leaves `feedback.txt` behind, an approving review removes it: otherwise a retry from `review` that passes would hand answered findings to a later develop round.

The retry itself and the prompts are unchanged: the existing retry note is already put in front of `feedback.txt`.

Alongside: the running-diagnosis mark is keyed by the daemon's data directory as well as the task number, because daemons sharing a test process refused each other's diagnoses. Running the new diagnosis tests one at a time was rejected: the existing ones collide the same way.
