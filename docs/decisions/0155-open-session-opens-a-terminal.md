# 0155: Open session opens a terminal through the CLI

The button runs `mason task open <id>` in a new Terminal rather than reimplementing the launch in the monitor: the CLI already resumes the agent's session in the worktree and records the intervention. The monitor asks `GET /tasks/{id}/session` first (through the websocket `raw` command, since the monitor has no other HTTP client) only to learn `idle`, so a running step gets a confirmation and `--anyway` is passed only on yes.

**CLI path.** `~/.config/mason/bin/mason`, never PATH. The daemon's reply does not carry its own CLI path; adding it was optional and would touch `http.rs`/`attention.rs`, so it is left for when the install location can differ.

**Decision in the core layer.** Both clients keep the decision (idle, confirm, argv, copy fallback, notice) in core/model code with injected `launcher` and `copier`, so tests need no GUI. Only the spawn is in the app layer.

**Fallback.** A launcher that returns false copies the full-path command and shows "Copied `...`; paste it in a terminal." next to the buttons. Compose cannot tell whether a long-lived terminal emulator started well: only a failure within 2 s counts.

Nothing in `PROTECTED` or `clients/features.toml` changed.
