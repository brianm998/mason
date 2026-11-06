# Spike: Suspend and resume

Status: measured (Claude Code 2.1.317 only; **Codex CLI not installed, not measured**)

## Question
Can each CLI end a turn on a pending long check and resume cleanly with its result? What happens to cost when the prompt cache expires during long waits?

## Method
`spikes/agent-cli/suspend_resume.sh`: Claude Code (haiku, scratch dir, `--strict-mcp-config`, one-shot `-p` calls) against `mcp_server.py`. Turn 1 calls `run_check`, whose reply says the check is pending and to end the turn. Then `--resume <session>` with a result note, immediately and again after 330 s.

## Result
```
== turn 1: call run_check, expect the turn to end
{"session": "6d58af29-bcbc-4f9e-b5ce-57bb89156447", "error": false, "turns": 3, "result": "SUSPENDED", "cost_usd": 0.0034083100000000003, "ms": 7574, "input": 6, "cache_read": 89211, "cache_create": 12208, "output": 148}
== waiting 0s
== turn 2: resume with result (wait was 0s)
{"session": "6d58af29-bcbc-4f9e-b5ce-57bb89156447", "error": false, "turns": 1, "result": "RESULT_OK: 12 tests executed, all green.\n\nThe check I started earlier was `unit`, via `mcp__spike__run_check`.", "cost_usd": 0.00398029, "ms": 4798, "input": 2, "cache_read": 33978, "cache_create": 780, "output": 152}
== waiting 330s
== turn 3: resume with result (wait was 330s)
{"session": "6d58af29-bcbc-4f9e-b5ce-57bb89156447", "error": false, "turns": 1, "result": "RESULT_OK: 12 tests executed, all green.\n\nThe check I started earlier was `unit`, run with `mcp__spike__run_check`.", "cost_usd": 0.00439947, "ms": 3452, "input": 2, "cache_read": 34758, "cache_create": 232, "output": 50}
```
- The turn ended cleanly with the agent's own "SUSPENDED" after one tool call. The process exited, so nothing was held during the wait.
- Resume kept full context: it named the earlier check (`unit`) and used the result.
- After a 330 s wait, past the 5-minute cache window, the resume still read 34,758 tokens from cache and created only 232 (immediate resume: 33,978 read, 780 created). No cache-expiry cost showed. Likely cause: a longer cache lifetime on this account; waits beyond that, and other accounts, were not tested.
- Cost per resume turn was about $0.004 (haiku).

## Conclusion
Suspend/resume works in Claude Code: end the turn by instruction in a tool result, free the slot, then `--resume` with a result note. A 5.5-minute wait did not lose the cache. Not yet known: waits of an hour or more, and the Codex CLI. Keep the held-session fallback available for Codex until it is measured (Stage 20).
