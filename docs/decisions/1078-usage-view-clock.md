# 1078: the unchanged-view test compares usage content, not absence

## Context
The usage view's text depends on the wall clock, so it can change with no change in MASON's state. `send_views` sends a view whenever its encoded bytes differ from the last one sent, which is correct.

## Decision
`an_unchanged_view_is_not_sent_again_and_a_changed_one_is` accepts a later usage view only if its content differs from the first one sent. An identical resend, the regression the test guards against, still fails.

## Alternatives
A fixed clock for the demo daemon's views: a product change made only for one test. Dropping the usage assertion: loses the check for an identical resend.

## Exit-script selection
The owner chose to narrow `scripts/select-exit-scripts.sh` in this change: a changed integration-test file selects only exit scripts naming `<stem>::`, not every script containing the module name as a word, so a `protocol.rs` edit no longer selects scripts that only mention "protocol" in prose.
