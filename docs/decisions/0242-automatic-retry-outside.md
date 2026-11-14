# 0242: retry failures outside the code automatically

Stage s247. Status: accepted by the owner.

## Context
161 retries in one week were all by hand; 32 followed failures that said nothing about the code. The only place a `Retried` event was made was the HTTP endpoint, and the classifier marked only landing failures and lost commands as outside.

## Decision
- Failures outside the code are classified as such (lost or never-started step host, failed or timed-out fetch, a CLI with no result, a refused `mason/ci/` push, as before landing and lost commands). Red rounds, weakenings, conflicts and agent submissions stay `code`.
- The first automatic retry comes 10 minutes after the failure, the second 60 minutes after the next failure; a third outside failure within 24 hours stays failed and raises the usual alert (the alert is held back only while a retry is still coming). Retries are counted in `auto_retries`.
- It goes through the endpoint's own function, with the same guard against a retry already done, and the event says `auto`.
- Three or more tasks failed with one normalised reason within ten minutes raise one host-wide alert; if the reason is a fetch, their retries wait until a fetch works.
- `auto_retry = false` in the project file turns it off.

## Not done
No probe other than fetch. Failures recorded before classes existed (no class) are not retried.
