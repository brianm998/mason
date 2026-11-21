# 0773: One shared build cache for remote runs, saved only from the base branch

Status: accepted (pending owner approval of the workflow files)

GitHub scopes an Actions cache to the branch that saved it, so every `mason/run/**` branch saved
its own and none could read another's: builds were cold and the repository sat at 10.8 GB of 10.

Decision: run branches restore a single key (`shared-key: mason-run`) and never save
(`save-if` develop). `mason-cache.yml` builds the workspace on develop pushes and daily and
saves it. The suite's target directory is symlinked to `./target` so that is what is cached.
The workflow sets up only the toolchains `run.json` names. Deleting a run ref also deletes its
caches. Not chosen: giving rust-cache the `mason-target` directory in `workspaces`, since the
symlink also covers the suite's `--target-dir` without depending on rust-cache's layout.
