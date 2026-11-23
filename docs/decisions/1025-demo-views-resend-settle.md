# 1025: demo views resend at each seq; settle waits for the seq

Cause (from code; not reproduced in the session that wrote it): `send_views` skips a view whose
hash is unchanged, so a client's seq can stay below the store's. Both settle loops therefore
kept a quiet-window fallback (1.5 s), which on a slow runner ended before the step's views
arrived, giving checkpoints that were sometimes ahead and sometimes behind (70 differences).

Change: in demo daemons only (`d.demo`), `send_views` resends an unchanged view when its seq is
newer than the last sent for that kind. Production keeps the dedupe. The conformance `settle`
and Compose `step` now wait for the step's seq with a 60 s deadline that fails loudly (decision
0763 wanted the wait for the step's seq; the fallback is gone).

Rejected: a longer quiet window (still a race, slower); loosening `compare` (hides real
differences).
