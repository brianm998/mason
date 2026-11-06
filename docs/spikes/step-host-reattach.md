# Spike: step-host reattach

Status: measured (macOS 15 / Darwin 24.6 locally; Linux via CI running `scripts/exit/s01.sh`)

## Question
Can a detached process keep a child running, journal its output and be reattached by a new controller after `kill -9`?

## Method
`spikes/step-host-reattach/`: `host.py` double-forks, calls `setsid`, owns one child, journals stdout to a file and serves a unix socket (`ATTACH <offset>` replays the journal from an offset, then streams live, then `EXIT <code>`). `controller.py` is the stand-in daemon and records what it received. `run.sh` starts a 40-line, 0.1 s/line child with exit status 7, `kill -9`s the controller after about 1 s, waits 1 s with nobody attached, starts a new controller, and diffs the received output against the expected lines.

## Result
```
controller killed after       10 lines; host alive: yes
no loss, no duplicates (40 lines)
exit status seen: 7
host session leader:  0 93479
STEP-HOST SPIKE PASS on Darwin
```
The host survived the controller's death in its own session, kept journaling while nobody was attached, and the new controller resumed from its byte offset with no loss, no duplicates and the correct exit status.

## Conclusion
The step-host design works on macOS, with plain `setsid` and unix sockets. A byte-offset journal is a sufficient resume protocol. Production must also: let the host exit once the exit status is delivered or acknowledged (the spike's host lingers), write the registry atomically, and handle a controller dying mid-frame (the offset is only advanced by whole frames written to disk). No fallback needed.
