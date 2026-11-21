# 0763: wait for the step's sequence number, not only a quiet window

Decision: the Compose conformance run waits for the seq the daemon reports after each step, and the model drops out-of-order older views. Rejected: a longer fixed quiet window (a timing guess that only moves the failure) and loosening `compare()` (weakens the check).
