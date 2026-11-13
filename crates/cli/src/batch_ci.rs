// SPDX-License-Identifier: GPL-3.0-or-later

//! The line `mason landing` prints under a batch that has CI.

use serde_json::Value;

/// `CI <ref>: 11 jobs: 1 queued, 2 running (static checks, Linux build), 7 passed, 0 failed
/// (14 min) <url>`; "queued for a runner" while no job has started. `ci` is the batch's `ci`
/// object as the daemon sends it (camelCase), `now_ms` the clock.
pub fn ci_line(ci: &Value, now_ms: u64) -> String {
    let n = |w: &Value, k: &str| w[k].as_u64().unwrap_or(0);
    let ws: Vec<&Value> = ci["workflows"].as_array().into_iter().flatten().collect();
    let sum = |k: &str| ws.iter().map(|w| n(w, k)).sum::<u64>();
    let (queued, running, passed, failed) =
        (sum("queued"), sum("running"), sum("passed"), sum("failed"));
    let names: Vec<&str> = ws
        .iter()
        .flat_map(|w| w["runningNames"].as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect();
    let mut s = format!("CI {}: ", ci["ciRef"].as_str().unwrap_or("?"));
    if running + passed + failed == 0 {
        s.push_str("queued for a runner");
        if queued > 0 {
            s.push_str(&format!(" ({queued} jobs)"));
        }
    } else {
        s.push_str(&format!(
            "{} jobs: {queued} queued, {running} running{}, {passed} passed, {failed} failed",
            queued + running + passed + failed,
            if names.is_empty() {
                String::new()
            } else {
                format!(" ({})", names.join(", "))
            }
        ));
    }
    let started = ci["startedMs"].as_u64().unwrap_or(0);
    if started > 0 && now_ms >= started {
        s.push_str(&format!(" ({} min)", (now_ms - started) / 60_000));
    }
    if let Some(u) = ci["url"].as_str().filter(|u| !u.is_empty()) {
        s.push(' ');
        s.push_str(u);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_running_batch_shows_counts_names_age_and_link() {
        let ci = json!({"ciRef": "mason/ci/star-b142-1", "url": "https://gh/run/9",
            "startedMs": 60_000u64, "state": "running",
            "workflows": [{"workflow": "pr-gate", "queued": 1, "running": 2, "passed": 7,
                "runningNames": ["static checks", "Linux build"]}]});
        assert_eq!(
            ci_line(&ci, 60_000 + 14 * 60_000),
            "CI mason/ci/star-b142-1: 10 jobs: 1 queued, 2 running (static checks, Linux build), 7 passed, 0 failed (14 min) https://gh/run/9"
        );
    }

    #[test]
    fn no_job_started_reads_queued_for_a_runner() {
        let ci = json!({"ciRef": "r", "workflows": [{"queued": 3}]});
        assert_eq!(ci_line(&ci, 0), "CI r: queued for a runner (3 jobs)");
        assert_eq!(
            ci_line(&json!({"ciRef": "r"}), 0),
            "CI r: queued for a runner"
        );
    }
}
