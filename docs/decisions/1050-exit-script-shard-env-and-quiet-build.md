# 1050: exit scripts clear the shard env and do not parse quiet cargo output

Decision: scripts that build a tmp repo without run-exit-scripts.sh unset `MASON_EXIT_SHARD`/`MASON_EXIT_PLAN` (as s249 does). Scripts needing a test binary read `executable` from `--message-format=json` instead of scraping `-q` output.

Why: the CI job sets the shard env for every script, and `-q` may print no Executable line.
