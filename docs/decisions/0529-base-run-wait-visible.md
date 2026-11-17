# 0529: Waiting for the base-run lock is a recorded wait

Status: accepted (s536).

Context: `BASE_RUN` allows one base check at a time. A task that found it held blocked in `lock()` with no event, so it read "running, no output" with no process, no slot and `wait: null` for as long as the holder took (25+ min seen on 2026-11-17).

Decision: the lock remembers its holder (task, what it runs at which base, since when). Taking it tries first; if it is held, the taker emits `WaitStarted` with class `base-run` and a text naming the holder, blocks, and emits `WaitEnded` once it has the lock. `wait_class` reads the text back to `base-run` for events without a class.

Consequences: the wait counts as a wait (it is credited against the step's time like other waits). What is held, and for how long, is unchanged; `profile` still holds the lock across its own lease waits.
