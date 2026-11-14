# 0247: refuse new timing-dependent tests at submit

Stage s252. Status: accepted by the owner.

## Context
"No timing assertions" is a rule in CLAUDE.md that nothing checked. At least eight stages and tasks dealt with timing-dependent tests, and one sched test flaked in ten tasks before it was fixed.

## Decision
The weakening scan, which runs at `submit_change`, gets the finding `timing-dependent-test`. It reads only added lines in test code and refuses the submission with file, line and rule for: a fixed sleep followed within five lines by an assertion; an assertion that compares `elapsed()` or an `Instant` difference with a constant; a fixed sub-second `recv_timeout`, `wait_timeout`, `wait_timeout_while` or `park_timeout`; in Kotlin `Thread.sleep(`/`delay(` before an assertion; in Swift `sleep(`, `usleep(` and `wait(for:timeout:)` under one second.

A line, or the one above it, with `// timing: owner <name> expires <YYYY-MM-DD>` and a date not yet past is let through. An expired marker is still a finding and says so.

A sleep inside a loop (opened within ten lines above) is a poll, not a fixed wait: waiting for a condition under a generous deadline is the sanctioned pattern and is not flagged.

## What the scan deliberately does not catch
- Anything written outside the added lines: a sleep already in the file and a new assertion after it, or the reverse (the diff is read with no context).
- Sleeps whose duration is a variable or a constant, and timeouts built from them.
- Elapsed time kept in a variable and asserted several lines later.
- Rust test code outside `tests/` that comes before the first added `#[test]` line of the diff.
- Test frameworks' own timeout arguments other than the ones named (`withTimeout`, `timeout =` annotations).
- Scripts and exit scripts (shell `sleep` followed by a check).

False positives were kept rare by flagging only the patterns above and by exempting loops; a miss is cheaper than a wrong refusal, because a refused submission costs a round.

## Checked against the repository as it is
The scan was run over every `.rs`, `.kt` and `.swift` file of the repository on 2026-11-13, each file fed as one diff with every line added (a throwaway test, not committed). The first rule set found 47 lines. Nine were this module's own test strings (text that looks like code), two were 5 ms pauses between two emits to give them different timestamps, one was a sleep inside a spawned thread whose block closed before the next assertion, and two were a deadline assertion and its 5 ms poll sleep in a loop whose opener was out of view. The scan now ignores the text of string literals, takes an elapsed-time assertion whose message says it is a timeout, and the sleep next to it, as a deadline check; ignores pauses of 10 ms or less; and stops looking for an assertion at a closing brace. That leaves 32 lines. 31 of them are real timing dependencies: fixed sleeps followed by an assertion, among them "nothing happens" waits of 300 ms to 4 s in `scheduling.rs`, `attention.rs`, `taskcache.rs`, `stall.rs` and `selfguard.rs`, and elapsed-time bounds of 3 to 30 s in `proc.rs`, `git.rs`, `kit.rs`, `mcp.rs`, `hosted_check.rs` and `fetch_deadline.rs`. One is a borderline false positive: `scheduling.rs:665`, a 20 ms pause that only separates two enqueue times. Existing lines are not refused: the scan reads only added lines, so these stay until someone touches them.
