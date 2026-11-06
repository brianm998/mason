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
