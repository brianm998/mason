# 0724: remote CLI dispatch by first argument

`mason remote setup` is routed to `remote::run`; every other `remote` command goes to the daemon handler. Error text for unknown subcommands comes from `remote::COMMANDS` so the two lists share one source.
