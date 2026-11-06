# 0002: Readings of Part 6 made in Stage 5

Where the design left room, Stage 5 chose the following. Any of them can be overruled.

- **Contract tools "through the step host".** The tools reach the agent through
  `mason-mcp`, a small MCP stdio bridge that runs in the agent's own process
  tree and forwards every call to a contract socket served by the controller
  (`ContractHandle::serve`). If the controller is briefly away (a daemon
  restart) the bridge retries for 30 s (`MASON_BRIDGE_RETRY_SECS`) instead of
  failing the call. The agent CLI itself is not yet launched under a step host:
  `ClaudeAdapter` builds an `AgentCommand` (pure) and launches it directly.
  Putting that command under a step host, and forwarding the contract socket
  through it, belongs with Stage 6, where the daemon exists and the
  `kill -9`-mid-session requirement is tested.
- **Who decides `Accepted`.** The contract sets a `SubmissionFlag` when a
  terminal submission is accepted; the adapter reads it when the turn ends.
  That keeps the design's six-variant `Outcome`. "Asked" and "suspended" are
  not outcomes: the turn ends on instruction, the adapter says `NoResult`, and
  the runner reads the contract's state to see it was deliberate.
- **Read-only steps.** `explore` and `review` get `Read,Grep,Glob` only;
  `develop` also gets `Edit,Write,Bash`. Read-only steps have no `run_check`
  or `submit_change`.
- **The prompt goes on stdin**, not argv: no length or quoting limits, and
  `--allowedTools` is variadic so a trailing positional would be swallowed.
- **Environment.** An allowlist (`PATH`, `HOME`, `USER`, locale, `TMPDIR`, …) plus
  `ANTHROPIC_*` and `CLAUDE_*`, which is provider configuration and includes an
  API key if the owner uses one. Everything else is dropped, notably all git
  host tokens, SSH agent and cloud keys. Git is further restricted with
  `GIT_ALLOW_PROTOCOL=file`, no askpass and no prompts.
- **`SessionHandle::cancel` became `canceller()`.** `cancel(&mut self)` cannot be
  called while another thread is blocked in `next_event`, which the conformance
  kit's hang scenario exposed. A `Canceller` can be called from any thread.
- **Token figures** come only from the final `result` event of each invocation.
  The stream repeats a message's usage on each of its content blocks, so
  summing per-message figures would double count; those are not shown.
  Rate-limit utilisation is passed through because the stream reports it.
- **`ask`** is validated minimally here (a question of at most 200 characters
  and two or more options). The full decision-card rules are Stage 10.
