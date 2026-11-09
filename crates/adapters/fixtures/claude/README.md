# Claude Code stream-json fixtures

Files named `real_*` were captured from Claude Code 2.1.317 running
`claude -p --output-format stream-json --verbose` (model haiku, scratch
directory). They are trimmed only to drop the large `tools`/`slash_commands`
lists in the init event and the opaque thinking `signature`, and the working
directory is redacted to `/scratch`.

Files named `synthetic_*` were written by hand to model situations that cannot
be provoked on demand (usage limit, provider outage, crash). Their shape
follows the real events above and Claude Code's documented `api_retry` event,
but they are **not captures**, and the tests label them that way.

`synthetic_not_logged_in.jsonl` is the one real-world shape we did see (the
`result` text comes from an actual run with a logged-out CLI), reproduced here
because the rest of that capture is empty.

## Usage fixtures (s62)

`real_plain.jsonl` and `real_tool_suspend.jsonl` carry the only real
`rate_limit_event`s: `unifiedWindows.five_hour` and `seven_day` with
`utilization`, `resetsAt` and `status`, and `overageStatus` rejected with
`overageDisabledReason: org_level_disabled`. A subscription capture has
`apiKeySource: none`.

Synthetic (hand-written, not captures; the field names follow the real events):

- `synthetic_usage_windows.jsonl`: five_hour rejected, with seven_day and seven_day_opus.
- `synthetic_usage_rejected_weekly.jsonl`: seven_day rejected.
- `synthetic_usage_overage_in_use.jsonl`: five_hour rejected while `isUsingOverage`.
- `synthetic_usage_overage_used_up.jsonl`: the same, with `overageStatus: rejected`.
- `synthetic_api_run.jsonl`: `apiKeySource: ANTHROPIC_API_KEY`, `total_cost_usd` 0.0123, no windows.
- `synthetic_api_rate_limit.jsonl`: an API account's 429 with no window.

The `apiKeySource` value on an API account and the overage field values
(`allowed`, `out_of_credits`, `overageResetsAt`) are assumptions; only the
`rejected` / `org_level_disabled` pair was seen.

## Partial messages (s76)

`real_partial_messages.jsonl` is the stream of `claude -p --verbose --output-format
stream-json --include-partial-messages` (Claude Code 2.1.317, two API messages, a `Read`
between). The **figures** are the ones measured on that run: `assistant` events carry
`output_tokens` 16 and 5, the `message_delta` events 137 and 851, and the `result` is input
4, output 988, cache read 7664, cache write 4503, which the deltas sum to exactly. **The
file itself was written by hand to that shape, not copied from the capture** (the
capture's text, ids and per-message cache split were not kept; the split of cache figures
between the two messages, ids, session id and text here are made up). Treat it as a
model of the stream, like the `synthetic_*` files, not a capture.

## Todo list fixture (s71)

`synthetic_todo_write.jsonl` is hand-written, **not a capture** (no `claude` CLI was
installed where it was made). It models a `TodoWrite` tool call with seven items (three
`completed`, one `in_progress`, three `pending`; each `{content, status, activeForm}`),
then an `Edit` and a `Bash` call. The shape follows Claude Code's documented `TodoWrite`
input. It should be replaced by a real capture when one can be made.
