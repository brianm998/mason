<!-- SPDX-License-Identifier: MIT -->
# Example workflows

Five starting points for `.mason/`. `mason init --workflow NAME` copies one into your
project. They are only a starting point: change anything, any time, in the monitor or in
`.mason/`. These files are under the MIT licence, so copying them creates no obligation.

| Directory | Steps | Landing | Good for |
| --- | --- | --- | --- |
| `solo-fast` | explore, develop, test, land | `direct` | side projects, low risk |
| `reviewed-pr` | explore, develop, test, review, land | `pr-each` (needs a git host) | open source, or working with others |
| `batched-ci` | as reviewed-pr | `pr-batch`, one integration PR per batch | slow or costly CI |
| `device-mobile` | adds simulator and emulator environments | `pr-batch` | iOS and Android apps |
| `human-gated` | explore, approve, develop, test, approve, land | `direct` | getting comfortable, sensitive code |

Each directory has `mason.toml` (the pipeline) and `landing.toml` (how work lands).
`mason init` fills in the `# suites:` marker line of the test step from the suites it
proposed, and takes the landing mode and base branch from your answers.

## `github/mason-run.yml`

Not a workflow pipeline: the GitHub Actions workflow MASON uses to run a suite or a check on a
GitHub-hosted runner (the `[remote]` section of a project file). Put it at
`.github/workflows/mason-run.yml` on the base branch and fill in its setup step.
