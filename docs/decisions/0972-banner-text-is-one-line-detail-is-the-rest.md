# 0972: A banner's `text` is one short line; `detail` holds the rest

Stage s979. The notification bar drew each banner's whole text, wrapped, so a commit message or a
two-sentence explanation filled it.

Decision:

- `Banner.text` is a one-line summary (no newline, at most 80 characters). `Banner.detail` is the
  rest: the reason, the error, what the owner can do, the task numbers. It may be empty.
- The daemon enforces this once, in `views::one_line` over every banner in `header`, rather than
  trusting each producer: a producer's own detail is kept; otherwise the whole text becomes the
  detail when it had to be cut. A new producer cannot break the bar.
- Clients draw the text on one line with tail truncation. A row with detail, or whose text was
  cut, gets a "?" button (accessibility label "Details") that opens a dialog with the summary, the
  detail and the row's actions. "Open task #N" is offered for each `#N` in the text or detail and
  uses the existing `mason://task/` route. Client-made rows (connection, read-only, updates) get a
  detail by the same rule.

Not decided here: hand-written summaries for the kinds that stay after the relevance work; the
automatic cut gives a readable line for them, not a hand-written one.
