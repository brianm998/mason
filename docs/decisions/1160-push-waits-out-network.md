# 1160: a candidate push waits out a network outage

A push that cannot reach the remote is the network's fault, not the code's. It waits and retries for up to 30 minutes, as checks already do (#772), then fails with the original error.
