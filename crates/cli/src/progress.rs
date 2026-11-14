// SPDX-License-Identifier: GPL-3.0-or-later

//! Live step progress on the command line: one line per running step, and `--watch`, which
//! follows the `StepProgress` pushes a monitor gets. The words come from
//! `mason_core::progress_text`, the same rule the monitors are checked against.

use crate::client::Daemon;
use mason_core::progress::Source;
use mason_core::progress_text::{count_text, group, time_text};
use mason_protocol::{server_message, ProgressCell, ProgressSource, ServerMessage, ViewKind};
use serde_json::Value;
use std::time::Duration;

fn source_of(v: &Value) -> Source {
    match v.as_str() {
        Some("reported") => Source::Reported,
        Some("counted") => Source::Counted,
        Some("history") => Source::History,
        Some("expected") => Source::Expected,
        Some("minimum") => Source::Minimum,
        _ => Source::None,
    }
}

/// One step's line from its JSON record (as `GET /tasks/{id}/progress` and the `progress` of
/// `GET /tasks/{id}` give it), against the daemon's clock `now_ms`:
/// `develop  212 of 340 tests, 62%; running the api suite; updated 4 s ago`.
pub fn line(p: &Value, now_ms: u64) -> String {
    let n = |v: &Value| v.as_u64().unwrap_or(0);
    let count = count_text(
        p["unit"].as_str().unwrap_or(""),
        n(&p["done"]["value"]),
        source_of(&p["done"]["source"]),
        n(&p["total"]["value"]),
        source_of(&p["total"]["source"]),
        n(&p["total_low"]),
    );
    let mut parts = vec![];
    if !count.is_empty() {
        parts.push(count);
    }
    if let Some(m) = p["message"].as_str().filter(|m| !m.is_empty()) {
        parts.push(m.to_string());
    }
    let time = time_text(now_ms, n(&p["updated_at_ms"]), n(&p["stale_after_ms"]));
    if !time.is_empty() {
        parts.push(time);
    }
    format!(
        "{}  {}",
        p["step"].as_str().unwrap_or("?"),
        parts.join("; ")
    )
}

/// The lines of every running step in a record set `{now_ms, runs: [...]}`, with the
/// sub-counts and todo list of each indented under it.
pub fn lines(r: &Value) -> String {
    let now = r["now_ms"].as_u64().unwrap_or(0);
    let runs = r["runs"].as_array().cloned().unwrap_or_default();
    if runs.is_empty() {
        return "no step is running".to_string();
    }
    let mut out = vec![];
    for p in &runs {
        out.push(line(p, now));
        let out_lines = p["output"]["lines"].as_u64().unwrap_or(0);
        let bytes = p["output"]["bytes"].as_u64().unwrap_or(0);
        if out_lines > 0 || bytes > 0 {
            out.push(format!(
                "    output {} lines, {} bytes",
                group(out_lines),
                group(bytes)
            ));
        }
        for s in p["subs"].as_array().into_iter().flatten() {
            if let Some(j) = s["jobs"].as_object() {
                let k = |n: &str| j.get(n).and_then(|v| v.as_u64()).unwrap_or(0);
                let names: Vec<String> = j
                    .get("running_names")
                    .and_then(|v| v.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect();
                out.push(format!(
                    "    {}",
                    mason_core::progress_text::jobs_text(
                        s["name"].as_str().unwrap_or("?"),
                        k("queued") + k("running") + k("succeeded") + k("failed"),
                        (k("queued"), k("running"), k("succeeded"), k("failed")),
                        &names,
                    )
                ));
                continue;
            }
            out.push(format!(
                "    {}: {}",
                s["name"].as_str().unwrap_or("?"),
                count_text(
                    s["unit"].as_str().unwrap_or(""),
                    s["done"]["value"].as_u64().unwrap_or(0),
                    source_of(&s["done"]["source"]),
                    s["total"]["value"].as_u64().unwrap_or(0),
                    source_of(&s["total"]["source"]),
                    0
                )
            ));
        }
        for t in p["todo"].as_array().into_iter().flatten() {
            out.push(format!(
                "    [{}] {}",
                t["status"].as_str().unwrap_or("?"),
                t["text"].as_str().unwrap_or("")
            ));
        }
    }
    out.join("\n")
}

fn source_json(s: i32) -> &'static str {
    match ProgressSource::try_from(s).unwrap_or(ProgressSource::SourceNone) {
        ProgressSource::SourceNone => "none",
        ProgressSource::SourceReported => "reported",
        ProgressSource::SourceCounted => "counted",
        ProgressSource::SourceHistory => "history",
        ProgressSource::SourceExpected => "expected",
        ProgressSource::SourceMinimum => "minimum",
    }
}

/// A pushed cell as the JSON record `lines` reads, so a pushed line and a fetched one read alike.
pub fn record_of(c: &ProgressCell) -> Value {
    let fig = |v: u32, s: i32| serde_json::json!({"value": v, "source": source_json(s)});
    serde_json::json!({
        "run": c.run, "step": c.step, "unit": c.unit,
        "done": fig(c.done, c.done_source), "total": fig(c.total, c.total_source),
        "total_low": c.total_low, "history_samples": c.history_samples,
        "stale_after_ms": c.stale_after_ms,
        "message": c.message,
        "output": {"lines": c.lines, "bytes": c.bytes, "last_output_ms": c.last_output_ms},
        "started_ms": c.started_ms, "updated_at_ms": c.updated_at_ms,
        "subs": c.subs.iter().map(|s| serde_json::json!({
            "name": s.name, "unit": s.unit,
            "done": fig(s.done, s.done_source), "total": fig(s.total, s.total_source),
            "jobs": s.jobs.as_ref().map(|j| serde_json::json!({
                "queued": j.queued, "running": j.running,
                "succeeded": j.succeeded, "failed": j.failed,
                "running_names": j.running_names}))})).collect::<Vec<_>>(),
        "todo": c.todo.iter().map(|t| serde_json::json!({"text": t.text, "status": t.status})).collect::<Vec<_>>(),
    })
}

/// Follow the `StepProgress` pushes of one task (`Some`) or of every task (`None`), printing a
/// line for each (or its full JSON record with `json`). Ends when the task is over, or when the
/// daemon goes away.
pub fn watch(d: &Daemon, task: Option<u64>, json: bool) -> Result<(), String> {
    use mason_protocol::client::Client;
    use mason_protocol::Encoding;
    let mut c = Client::connect(d.port, Encoding::Json)?;
    c.hello(0, false)?;
    let (views, id) = match task {
        Some(t) => (vec![ViewKind::TaskDetail], t),
        None => (vec![ViewKind::Sidebar], 0),
    };
    c.subscribe(&views, "", id)?;
    loop {
        let Some(ServerMessage { body: Some(b) }) = c.recv(Duration::from_secs(1))? else {
            continue;
        };
        match b {
            server_message::Body::StepProgress(m) => {
                let Some(cell) = m.progress.as_ref() else {
                    continue;
                };
                let rec = record_of(cell);
                if json {
                    println!("{}", serde_json::to_string(&m).map_err(|e| e.to_string())?);
                } else {
                    let prefix = if task.is_none() {
                        format!("#{} ", m.task)
                    } else {
                        String::new()
                    };
                    println!("{prefix}{}", line(&rec, m.now_ms));
                    for l in &m.log {
                        println!("    | {l}");
                    }
                }
            }
            server_message::Body::View(u) => {
                // A task that is over has nothing more to say.
                if let Some(mason_protocol::view::Body::TaskDetail(t)) = u.view.and_then(|v| v.body)
                {
                    if task.is_some()
                        && t.row.as_ref().is_some_and(|r| {
                            matches!(r.state.as_str(), "landed" | "failed" | "canceled")
                        })
                    {
                        return Ok(());
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rec(done: (u64, &str), total: (u64, &str)) -> Value {
        json!({"step": "develop", "unit": "tests",
               "done": {"value": done.0, "source": done.1},
               "total": {"value": total.0, "source": total.1},
               "total_low": 0, "message": "", "stale_after_ms": 300000,
               "updated_at_ms": 100000, "output": {"lines": 0, "bytes": 0}})
    }

    #[test]
    fn real_total_shows_a_percentage() {
        let l = line(&rec((212, "reported"), (340, "reported")), 104_000);
        assert_eq!(l, "develop  212 of 340 tests, 62%; updated 4 s ago");
    }

    #[test]
    fn history_total_shows_the_range_and_no_percentage() {
        let mut r = rec((1240, "counted"), (2400, "history"));
        r["unit"] = json!("lines");
        r["total_low"] = json!(1800);
        let l = line(&r, 104_000);
        assert_eq!(
            l,
            "develop  1,240 lines, usually 1,800 to 2,400; updated 4 s ago"
        );
        assert!(!l.contains('%'));
    }

    #[test]
    fn no_total_shows_the_count_and_a_quiet_step_says_so() {
        let mut r = rec((37, "reported"), (0, "none"));
        r["unit"] = json!("tool calls");
        r["message"] = json!("reading the code");
        assert_eq!(
            line(&r, 100_000 + 360_000),
            "develop  37 tool calls; reading the code; no output for 6 min"
        );
    }

    #[test]
    fn a_set_lists_sub_counts_and_todo() {
        let mut r = rec((3, "reported"), (7, "reported"));
        r["subs"] = json!([{"name": "unit", "unit": "tests",
            "done": {"value": 4, "source": "reported"}, "total": {"value": 8, "source": "reported"}}]);
        r["todo"] = json!([{"text": "Read it", "status": "done"}]);
        let s = lines(&json!({"now_ms": 101_000, "runs": [r]}));
        assert!(s.contains("    unit: 4 of 8 tests, 50%"), "{s}");
        assert!(s.contains("    [done] Read it"), "{s}");
        assert_eq!(
            lines(&json!({"now_ms": 1, "runs": []})),
            "no step is running"
        );
    }
}
