# 0106: Alert when git fetch keeps failing

The background fetch discarded its errors, so an expired token or removed remote left the base stale silently, and #66 stops `prepare` from surfacing it. The record lives in `fetch_remote` (every fetch, not only the watcher's) as `(first failure ms, last error)` per project; success removes it. `alerts_now` raises `fetch:<project>`, kind `tracker`, once the failure is older than `fetch_alert_after` (15 min; a setting so tests shorten it). The clock runs from the first failed fetch, not the last, so a steady failure does not restart it. Only the record is read by the alert pass.

Kind `tracker` was chosen by the task; it routes like the tracker alerts. `crates/daemon/src/attention.rs` is a trust file: this adds one alert and changes nothing about who may answer.
