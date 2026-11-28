# 1236: prompt eval results are information for the owner, not a gate

Decision: `mason prompts eval` only reports (summary line, per-case judgements, a link on the owner-approval hold). No landing, test or review step reads it, and it never runs in the test step or landing. Why: a grader's judgement is itself a model output, so it can inform the owner's approval of a prompt change but cannot replace it. The CLI runs the sessions itself rather than through the daemon's broker, so it needs no new daemon route; the cost is that it takes no `llm:claude` slot (see s1246).
