# 0074: Alerts run on their own thread, and say when git is stuck

Alerts and notifications run on an attention thread that never waits on git or the repository lock; the watch, fetch and admission stay on the definitions thread. Speech is off that thread with a time limit per sentence. Stuck git work is itself an alert, from a "last pass ended" time, so the owner hears of it even when nothing else can.

`crates/daemon/src/attention.rs` is a trust file: this adds one alert (`git-stuck`) and changes nothing about who may answer. `crates/core/src/notify.rs` gets one line: `git_stuck` sounds as a broken thing. Neither touches a gate, guard, budget or verdict.
