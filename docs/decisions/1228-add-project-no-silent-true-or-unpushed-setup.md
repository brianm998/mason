<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1228: Add Project refuses a silent `true`, says when the setup is unpushed, and asks where runs go

**Decision.** (1) With suites accepted and none proposed or kept, Add Project is refused until the
owner gives `--test COMMAND`; `--test true` is the explicit "no tests". `--suites skip` stays an
explicit answer. (2) With a remote, a landing that leaves `.mason/` off `<remote>/<base>` is a
warning before Add, naming the push; the proof task is withheld until that ref has it, and doctor
warns when the local base has `.mason/` and the remote base does not. The default landing stays
`no`. (3) With `[nodes] enabled`, `placement` is asked and written as `[placement]`; the
recommendation is the mode of the most recently registered nodes-only project, else `local`.

**Why.** Each of the three let Add finish "successfully" with a setup that was unused or proved
nothing. Suites are also looked for one folder down, because that is where Swift, Cargo, Go and
Gradle packages often are.

**Rejected.** Pushing for the owner (never without their answer). Refusing `--suites skip`
(every fixture and script uses it, and it is an answer). Searching deeper than one folder (slow,
and finds vendored code).
