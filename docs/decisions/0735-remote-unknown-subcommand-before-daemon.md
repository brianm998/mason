# 0735: Validate the remote subcommand before the daemon lookup

`mason remote bogus` died with "masond is not running" because the daemon lookup ran before the subcommand match (#754).

**Chosen.** An early check in `main.rs`, beside the `remote setup` route, rejects unknown subcommands with the command list. `on`, `off`, `status` and no argument fall through unchanged.

**Not chosen.** Moving the daemon lookup later would touch every command.
