# Spike: Suspend and resume

Status: NOT MEASURED. Blocked, see below.

## Question
Can each of Claude Code and Codex CLI end a turn on a pending long check and resume cleanly with its result? What happens to cost when the prompt cache expires during long waits?

## Method (prepared, not yet run)
`spikes/agent-cli/suspend_resume.sh` runs Claude Code (haiku, scratch directory, `--strict-mcp-config`) against `spikes/agent-cli/mcp_server.py`. Turn 1 calls `run_check`, whose reply tells the agent to end its turn. Then it resumes the same session with a result note immediately and after 330 s (past the 5-minute cache window), printing `cache_read` and `cache_create` tokens for each turn.

## Result
None. The `claude` CLI in the agent's environment reports `loggedIn: false` (`Not logged in · Please run /login`), so no LLM call could be made. I did not log in or bypass the sandbox. The Codex CLI is not installed on this machine, so the Codex half cannot run either; installing it is a download I did not make without approval.

## Conclusion
Unknown until the script is run by someone logged in. Run `bash spikes/agent-cli/suspend_resume.sh` from a logged-in terminal and paste the output here. Fallback if suspend/resume fails: a held session that does not count toward the slot limit (as the plan specifies).
