# 0781: one header per batch, with a state, severity and times

Status: accepted (display and proto only; no verdict logic changes)

Decision: the landing view carries the batch as one line (`BatchLine`) with state text, severity ok/warn/stuck plus a reason in words, time in phase, and the median time of the last 30 batches for that phase when at least three samples exist. Phase entries are recorded in `batch_phases`; the verification round in `batches.round`. Protocol version 2; version 1 clients are still accepted. Stuck means time in phase over three times the usual (30 min without history), or held/frozen.

Not verified: see docs/stages/s788.md.
