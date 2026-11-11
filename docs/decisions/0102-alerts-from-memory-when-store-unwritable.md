# 0102: alerts are held in memory while the store cannot be written

Status: accepted

The disk-full alert is itself a store write, so on a full disk it could never be shown or sent.

Decision: when writing the alert table fails, the daemon keeps the current alerts (with their start times) in memory and serves and delivers from there; deliveries it cannot record are kept in memory too, so each channel is sent at most once per alert. When writes work, `sync_alerts_since` inserts the held alerts with their original times and the held deliveries are recorded; nothing is announced again.

Alternatives: a second small file store for alerts (more state to keep consistent; rejected); injecting failure inside the SQLite layer (a daemon-level switch is enough and simpler).

Consequences: sound/speech announcements wait for the store (they live in its queue). `attention.rs` is a trust file; the change is a one-line delegation and does not touch who may answer. Needs owner approval as a trust-file change.
