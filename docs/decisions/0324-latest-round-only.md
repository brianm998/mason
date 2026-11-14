# 0324: only the newest announcement of an alert waits

Each reminder round is a new announcement key, so rounds queued while the monitor was muted all replayed on unmute. Decision: the store treats `alert:<key>:<since>:` as the alert run; queuing a new announcement expires untaken ones of the run, and `take_announcements` hands over one per run and never a reminder for a run already played. The decision lives in the store, so both monitors behave the same. No spoken summary was added (a possible follow-up).
