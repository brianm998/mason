# 0323: Copy worktree and Copy branch name are labelled bordered buttons

The owner did not notice the small muted "branch" and "worktree" links. They are now bordered buttons labelled "Copy branch name" and "Copy worktree" with the copy icon. After a click the label reads "Copied" for 1.5 seconds. The key's copy button stays as it was. The logic (labels, hiding, label swap) lives in a plain `CopyTarget` type so tests need no timing and no click simulation.
