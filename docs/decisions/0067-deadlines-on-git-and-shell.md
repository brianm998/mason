# 0067: Every git and shell call has a deadline

One stuck `git` held the per-repository lock and stopped the pipeline silently. Every child process the daemon starts for git, builds, the resolver, the status script and `top` now has a deadline, runs in its own process group, and is killed with the whole group when it expires. The caller gets an error that says it timed out (`GitError.timed_out`, `ProcError::TimedOut`).

| call | deadline | why that is long enough |
|---|---|---|
| `git::run` | 5 min | local git calls take milliseconds; a fetch or push of this project's size takes seconds. 5 minutes is a hundred times that. |
| `git clone` | 30 min | moves the whole history, once. |
| `update::shell` | 1 hour | the self-update build (a release `cargo build` of the workspace) is the longest call. |
| `Batch::resolve` | 1 hour | the resolver may be an agent working through a conflict. |
| `regenerate_status` | 5 min | a script that rewrites one block; well under a second. |
| `top` | 10 s | takes about two seconds. |

**What I did not verify.** I did not measure the slowest calls in real logs: `masond.log` records no durations for them, and I did not mine the event store. The values are judgments from what each call does, not from measurement. They are all configurable (`MASON_GIT_TIMEOUT_SECS`, `MASON_UPDATE_SHELL_TIMEOUT_SECS`, `MASON_RESOLVER_TIMEOUT_SECS`) so a call found too slow can be given more without a code change; the owner should check them against real timings before relying on "no healthy call is ever killed".

**Trust.** `crates/landing/**` and `update.rs` are guards. This adds limits and raises none: nothing is allowed that was refused, no gate or verdict logic changed. The one behaviour change in a guard path is that a call that used to hang for ever now fails at its deadline.

Other choices:
- Network git gets `http.lowSpeedLimit=1`, `http.lowSpeedTime=60`, and ssh `BatchMode`/`ConnectTimeout`, so a stalled connection ends on its own before the deadline does.
- Fetches take a lock of their own, not the repository lock, so a slow fetch holds up only other fetches. A fetch that started while a caller waited for the lock counts for that caller (it saw everything the caller could have wanted); a fetch that finished earlier does not, so nothing is ever stale.
- Startup reads what is already fetched; the watcher's first pass fetches. The port opens before any network.
- A CPU reading older than 30 s is unknown, which means no gate, rather than the last value for ever.
- A killed child is reaped on a thread, so a process frozen at launch does not hold the caller.
