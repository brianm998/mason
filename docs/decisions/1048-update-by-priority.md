<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1048: masond updates itself by priority

## Decision

When the channel has a newer build, masond schedules the update itself. Its priority comes from what `running..target` contains: urgent (an urgent or fix task landed, an `Update-Urgent:` line, or an owner mark) goes as soon as running landing batches finish; normal (daemon code changed) waits until a takeover interrupts nothing or `normal_max_wait_s` passes; off-peak (CLI, tests, scripts) waits for the window or a quiet machine; none (docs, clients, markdown) causes no takeover. A newer build replaces a waiting one with the higher priority of the two.

## Why

Every takeover so far was started by hand, right after something landed, and each restarted checks and test steps, often for changes the daemon did not run. Classifying by content and timing by cost removes the avoidable restarts and lets urgent fixes go without someone noticing.

## Left open

The cost counts agent checks without a call id, test steps and batches equally; a weighted cost could follow. The release channel is not scheduled. The priority of a range is by paths, so a task that only touches docs but is marked urgent causes no takeover.
