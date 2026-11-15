# 0326: monitors decode off the main thread and coalesce progress

Status: accepted (s332)

**Decision.** Frames are decoded on a background task/thread; only finished messages reach the model on the
main/UI thread, in order. `StepProgress` is held and applied once per frame (~30 ms); any other message flushes
held progress first so ordering is preserved. File reads (`daemon.json`, the terminal file) are off the UI thread.

**Why.** A busy daemon sends large snapshots and bursts of progress; JSON decoding and one sidebar rewrite per
tick froze the UI. Compose also called `apply` from the socket thread while the UI read the model.

**Cost.** Progress can show up to one frame late. Protocol unchanged.

**Not verified.** Nothing was built or run in this environment.
