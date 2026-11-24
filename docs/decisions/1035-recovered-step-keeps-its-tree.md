<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1035: a recovered step keeps its tree

## Decision

Mark a prepared run with `prepared-r<run>` and let a job that finds the mark skip the base merge. Agents skip it unconditionally; test steps only while the worktree still has the recorded tree.

## Why

A restart usually follows a push to develop. Merging then changed the tree under a live agent and invalidated a test step's finished suites. Picking up develop belongs to a fresh run or a new check.

## Left open

A job restarted inside one daemon for the same run id also counts as recovered. The base-red wait and deliberate folds are untouched.
