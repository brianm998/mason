# 0321: the CPU history lives in the daemon's memory and is served as a range

**Decision.** The daemon keeps the CPU readings in a ring (`crates/daemon/src/cpuhistory.rs`): the last hour as sampled, 24 hours as one-minute means. It is **not persisted**: a daemon restart starts the graph afresh. A monitor asks for a range with `GET /cpu/history?span=` or by subscribing to `CPU_HISTORY` with `Subscribe.cpu_span_ms`, and does so only while Home is on screen; the home view never carries the history.

**Why.** About 1,440 minute points plus 720 fine ones, each with a dozen shares, is under a megabyte; writing it to disk every 5 s would add disk traffic for a graph whose loss on restart costs nothing the owner cannot see again within minutes. Thinning old data keeps the 24 h answer small enough to send.

**Trust.** `clients/features.toml` is in `PROTECTED`; the new `cpu-history` row waits for the owner's approval.
