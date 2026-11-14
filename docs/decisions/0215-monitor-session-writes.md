# 0215: Monitor sessions write only to their own socket

The shared `socket` field let an old session's ping or subscription reach a new socket before its hello, and a hello could be dropped if `onOpen` fired before the socket was stored. Each session now keeps its own socket and the hello waits for it. The ping is sent before the stall check, so a late tick pings and then stalls; the stall test's ping assertion no longer depends on the ping/stall ratio.
