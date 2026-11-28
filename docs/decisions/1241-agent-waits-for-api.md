# 1241: an unreachable API is waited out, not failed

Decision: an agent end that says the API could not be reached resumes after the network backoff, bounded by `network_agent_hold`, then fails as `Outside`. Why: the outage is the machine's, so it is no failure of the code, and a diagnosis session cannot reach the API either. Recognised by wording, not a new adapter field. `crates/core/src/task.rs` (protected) changed by one classification line; owner approval needed for that. No gate, guard or budget weakened.
