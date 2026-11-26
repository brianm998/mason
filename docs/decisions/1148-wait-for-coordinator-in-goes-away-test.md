# 1148: wait for the coordinator's run record, not the node's job count

The node counts a job as running before the coordinator has the reply to its job request. A test that stops the listener at that moment can make the coordinator fail while placing the job. The goes-away test now waits for the coordinator's own run view to show the run running on node b. Long coordinator names are handled by `coordinator_key` (8aec915bd).
