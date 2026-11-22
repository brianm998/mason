# 0852: the fake host runs checks through the project shell

## Context
CI/windows was red on two fake-host tests because `checks_of` ran `sh` and tar with Windows paths, and cached the failure as a red result.

## Decision
Use `shell_command` and `to_shell_path`, as every other project command does. Cache only a real run's result.

## Consequences
Unix behaviour is unchanged. A transient failure to archive is retried on the next call instead of being remembered.
