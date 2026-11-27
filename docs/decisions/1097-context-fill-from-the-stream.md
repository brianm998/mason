# 1097: context fill is read from the stream, kept in tokens per session, and only reported

## Question

How should MASON record how full an agent's context window got, and what counts as a send-back
when the report compares fill with outcomes?

## Decision

- **Source.** The Claude adapter takes the input of each main-conversation message (uncached,
  cache read and cache write) from `message_start`, and from the `assistant` line's `usage`,
  which carries the same input figures and is all a capture without partial messages has. A
  subagent's messages (`parent_tool_use_id` set) are not counted. The `result` line's `usage` is
  a sum over messages and is never used for fill.
- **Window.** `contextWindow` from `system/init` or a `result`'s `modelUsage`. Before either, an
  assumption: 1,000,000 for `claude-haiku-5-5` (captured) and names containing `[1m]`, else
  200,000, raised to 1,000,000 when more than that is in use. The step keeps the largest window
  its events carried, and the store keeps tokens, so a window stated at the end corrects the
  percentages of the whole session.
- **Compaction.** Any `system` line whose subtype contains `compact`. The subtype is carried in
  the event. The sizes are optional: the size afterwards is usually not known when the line
  arrives.
- **Storage.** A table of its own, `context_runs`, keyed by attempt and session, beside
  `usage_runs`: a step with a compaction but no tokens still gets a row, and a session written
  twice (journal replay, resume) merges by maximum instead of counting twice.
- **Send-back.** A `SentBack` event counts against the latest run, at that moment, of the step it
  sends the work back to. So the rate says how often a session's work was rejected. A review or
  explore row shows send-backs only if work is sent back *to* that step.
- **Buckets.** A step run that compacted is in `compacted` only; the others are split by peak at
  50, 75 and 90 percent. A step run's peak is the highest over its sessions.
- **No restart.** Nothing acts on the figures. An automatic restart with a summary waits for the
  owner to read this report.

## Not chosen

- Columns on `usage_runs`: that row is replaced whole and skipped when no tokens were reported.
- Storing percentages: an assumed window would be frozen into them.
- Attributing a send-back by timestamps: the event log already says which run was open.
