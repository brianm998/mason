# 1141: write-time rules are judged by the daemon, from the base commit

- **The hook only forwards.** `mason hook edit` sends its input over the step's contract
  socket; the daemon reads the rules from the project's own repository at the base ref, matches
  and records. One place reads the rules, the worktree cannot change them, and hits need no
  second channel.
- **Fails open**, unlike the command guard (1096): a rule is advice that also stops known
  mistakes, tests and review still follow, and a broken rules file must not stop every edit.
- **One rule per file.** A broken file disables that rule only.
- **Patterns see the edited file, and count only on added lines.** Matching the added text
  alone would miss a changed line inside a statement whose first line stayed (rustfmt's
  multi-line `assert!(`).
- **`cfg_test_paths`** was added to the task's fields: "timing assertions in `#[cfg(test)]`
  code" cannot be said with path globs and a regex. Braces are counted outside comments and
  strings.
- **Hits are a file in the task's directory**, not a table: they are only read back for the
  task they belong to.
