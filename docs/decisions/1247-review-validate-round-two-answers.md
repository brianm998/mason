# 1247: review_validate round-2 approvals answer round 1

The round-2 rule (task 1135) requires a review after a send-back to answer every open blocking finding in `earlier`. The three failing `review_validate` tests approved in round 2 without it, so the approval was refused. The tests now answer finding 0 as `fixed`. Production code is left alone: the rule is right and the tests were stale, so this is not a gate weakening.
