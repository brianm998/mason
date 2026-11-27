<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1213: Add Project resolves the agent binary and tokens as the daemon sees them

Add Project runs inside masond, so what it detects is what masond can run. Setup now always
writes an absolute `claude_bin`/`codex_bin`, found from `--agent-bin`, then PATH, then the binary
another registered project of that provider uses, then `~/.local/bin`; only an executable file
counts. With none, the plan asks (`agent-bin`) and `setup`/`apply` refuse before writing anything,
so no proof task is filed for a project that cannot start an agent.

For the GitHub sections, the `env:` token variable is checked in the process that will read it.
Unset: a `warnings` entry and a `token` question (`file:`/`env:` reference, or `none` to leave
the sections out). Setup never writes a literal token and never borrows another project's token
file; the reference is the owner's choice (`--token`). An unanswered unset variable is still
written as `env:GITHUB_TOKEN`, with the warning, so a non-interactive apply is not blocked on it.

`POST /onboarding/plan` and `/onboarding/apply` return `warnings`. The hints that differ between
the daemon and a test (PATH, HOME, environment, known binaries) travel in `Flags.machine`, which
is not part of the wire format.
