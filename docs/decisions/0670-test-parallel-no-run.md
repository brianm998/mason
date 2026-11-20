# 0670: test-parallel drops a duplicate --no-run

The suite build is `{project_test} --no-run`. With the parallel runner as `test`, `split_cargo` kept
that `--no-run` and `list_executables` appended a second, so cargo failed with
"Unrecognized option: 'no-run'". Decision: `split_cargo` drops any `--no-run`; the runner adds exactly one.
The build line is unchanged.
