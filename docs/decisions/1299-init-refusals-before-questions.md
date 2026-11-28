# 1299: `mason init` checks the repository before the questions

**Decision.** The CLI refuses a repository with no commits, then a base branch that does not
exist, before it resolves any question. Both checks live in one function, `repo_refusal`, which
`setup` also calls, so the two cannot drift.

**Why.** Nothing an owner answers can fix an empty repository, and a refusal about the test
command in its place sends them to the wrong problem.

**At a terminal** the base is checked early only when `--base` gave it. The detected default may
not exist (a detached HEAD with no `main`), and the owner can answer another.

**A base that is not a plain name** is left to the base question, which says so.

**Not chosen.** Adding `--suites skip` to the two test cases would have hidden the order
problem.
