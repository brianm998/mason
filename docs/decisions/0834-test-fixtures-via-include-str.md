# 0834: test fixtures in mason-core load via include_str!

Decision: the ci_watch test helper `fixture()` embeds the logs with `include_str!`
instead of reading them with `std::fs`. `scripts/check-pure.sh` stays unchanged and
keeps rejecting IO anywhere under crates/core/src; teaching it to skip `#[cfg(test)]`
modules would weaken the gate.
