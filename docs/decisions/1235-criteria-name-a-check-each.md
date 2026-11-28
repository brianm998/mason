# 1235: acceptance criteria name a check each

Decision: criteria become `{criterion, check}`; develop reports a status per criterion and review grades each. Old string criteria stay readable and are not graded. Why: a missed deliverable was found only by review or the owner (trd #1112). Develop cannot edit criteria; `request_exploration` is the way out. No gate, limit or guard weakened; `contract.rs` and `suites.rs` are protected and need owner approval.
