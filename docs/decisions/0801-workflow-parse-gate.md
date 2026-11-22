# 0801: Parse every workflow in the lint tier

Task #825. #784 landed an unparseable `ci.yml` because testing was skipped, leaving develop with no CI.
Decision: quote the run-name, and add `scripts/check-workflows.sh` to `suite.lint` (which still runs for
fast-tracked tasks). No YAML crate was added, to avoid a dependency and lockfile change; the script uses
ruby Psych or python3 yaml and fails if neither exists. The owner approved the ci.yml fix and the added gate.
