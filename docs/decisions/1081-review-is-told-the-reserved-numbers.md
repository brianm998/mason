# 1081: the review prompt states the task's reserved numbers

## Decision

The review prompt names the task's reserved stage note, decision record and exit script, says their presence is required, and asks the reviewer to judge their content only. The text lives in `numbers.rs`, next to `prompt_block`, and is appended in `jobs.rs` after pack or built-in selection. Review reads the stored reservations and never reserves.

## Why

The landing step already enforces the numbers (`check_numbers`), so a reviewer has no reason to police them. `prompts.rs` is a protected file and does not need to change.

## The analyst's develop step has no change validator

The analyst's result is an uncommitted file and its branch is never merged, so refusing its `submit_change` for a dirty tree or no commits only kept its findings from being read (s22 red since cb040fac3). Its develop step gets no validator; every other develop step keeps it. Exempting the result file alone was not chosen: the no-commits proof would still refuse it.

## Not verified

That a real reviewing model follows the text.
