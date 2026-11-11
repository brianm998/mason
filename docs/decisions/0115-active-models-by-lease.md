# 0115: Active models is driven by granted leases

The list answers "which sessions are running", and the counter beside it counts granted LLM leases, so the list is built from the same fact: `Broker::llm_held_by`. Step state alone was rejected because a step waiting for a slot has a model but no session. `has_lease` was rejected because it is true for a queued lease too. A held lease whose row has no model is still listed, under the display name or step, so a real session is never hidden.
