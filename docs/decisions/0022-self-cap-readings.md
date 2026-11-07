# 0022: Readings made while changing the self-hosting weekly cap (s44)

This changes `crates/daemon/src/selfguard.rs`, a trust file; it loosens a guard and waits for the owner.

- **Why it is safe enough.** The cap exists to stop a loop in which MASON files work for itself without limit. That loop's work arrives as agent proposals (`OwnerAgent`) and outside issues the owner labeled (`Outside`); both still count toward `per_week` exactly as before. A person filing their own tasks is not that loop, can see how much they file, and is already bounded by `max_open`.
- **Plainly: an owner identity can still file without a weekly cap.** So can any client that reaches the HTTP API, since `POST /tasks` files as Owner, and a tracker issue by an owner name. This is a loosening, accepted by this change.
- **Guards that remain:** `max_open` (open tasks only, so file-and-cancel churn is not bounded by it), `max_steps` (agent steps per period, the spend cap), the cooldown after a rollback (stops every filing, the owner's too), and the holds on protected files (a self-change touching a gate, guard, budget, verdict or trust file still waits for the owner's approval of that exact diff).
- **Cancelled before start.** A task cancelled with no attempts (no agent step began) does not count: it spent nothing. One that started and was then cancelled still counts.
- **Where the line is.** Trust `Owner` is "yours": the CLI, the monitor, the tracker import of an owner-named author, the rollback proposal and the landing freeze fix. Counting is by trust at filing, not by who later approves.
- **Compatibility.** `Status` keeps its four fields; `check` and `check_filing` stay strict (week applies to all). New: `check_as`, `check_filing_as`, `usage`, `count`. `GET /self` keeps `week` (now the counted number) and adds `yours_not_counted`.
- **Not verified.** Real GitHub; whether agent runs can reach the HTTP API and so file as Owner; monitors (nothing reads `/self`); no daemon path files `OwnerAgent` today, so the flood tests file it through the event directly and the exit test uses outside issues.
