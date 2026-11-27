// SPDX-License-Identifier: GPL-3.0-or-later

//! What the reviewer found (task 1132). Only a `blocking` finding sends a task back. The
//! `minor` ones are advice: kept per task for `mason task show` and the monitors, listed in
//! the feedback of a send-back, and, after an approval, given to the next develop round if
//! there is one.

use serde_json::{json, Value};
use std::io::Write;
use std::path::Path;

/// One JSON object per line: `{"run", "issue", "file"}`. The minor findings, oldest first.
const FILE: &str = "review-findings";

/// An approval's advice that no develop round has acted on yet.
const ADVICE: &str = "review-advice.txt";

fn of_severity<'a>(findings: &'a Value, severity: &'a str) -> impl Iterator<Item = &'a Value> {
    findings
        .as_array()
        .into_iter()
        .flatten()
        .filter(move |f| f["severity"] == severity)
}

/// `issue (file)`, the file only when there is one.
fn text(f: &Value) -> String {
    let file = f["file"]
        .as_str()
        .filter(|p| !p.is_empty())
        .map(|p| format!(" ({p})"))
        .unwrap_or_default();
    format!("{}{file}", f["issue"].as_str().unwrap_or("").trim())
}

fn bullets(findings: &Value, severity: &str) -> String {
    of_severity(findings, severity)
        .map(|f| format!("\n- [{severity}] {}", text(f)))
        .collect()
}

/// The feedback of a `changes` verdict, each finding marked with its severity.
pub fn changes_feedback(summary: &str, findings: &Value) -> String {
    let mut s = format!(
        "The reviewer asked for changes: {summary}\nBlocking findings (they sent the task back; fix them):{}",
        bullets(findings, "blocking")
    );
    let minor = bullets(findings, "minor");
    if !minor.is_empty() {
        s.push_str(&format!(
            "\nMinor findings (advice; they did not send the task back):{minor}"
        ));
    }
    s
}

/// The advice an approval carries, if it has any minor finding.
pub fn advice(findings: &Value) -> Option<String> {
    let minor = bullets(findings, "minor");
    (!minor.is_empty()).then(|| {
        format!("The reviewer approved an earlier version of this change with minor findings (advice; none of them has to be fixed):{minor}")
    })
}

/// Keep a review's minor findings. One already kept is not kept twice.
pub fn keep(dir: &Path, run: u64, findings: &Value) {
    let mut seen: Vec<String> = list(dir).iter().map(text).collect();
    let mut lines = String::new();
    for f in of_severity(findings, "minor") {
        let t = text(f);
        if !seen.contains(&t) {
            let kept = json!({"run": run, "issue": f["issue"], "file": f["file"]});
            lines.push_str(&format!("{kept}\n"));
            seen.push(t);
        }
    }
    if lines.is_empty() {
        return;
    }
    let written = std::fs::create_dir_all(dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(FILE))?
            .write_all(lines.as_bytes())
    });
    if let Err(e) = written {
        tracing::warn!(dir = %dir.display(), "minor review findings could not be kept: {e}");
    }
}

/// Every kept finding, oldest first.
fn list(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(dir.join(FILE))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// One line for `mason task show` and the monitors: `minor: issue (file)`.
pub fn lines(dir: &Path) -> Vec<String> {
    list(dir)
        .iter()
        .map(|f| format!("minor: {}", text(f)))
        .collect()
}

/// Keep an approval's advice for the next develop round.
pub fn keep_advice(dir: &Path, advice: &str) {
    let mut all = pending_advice(dir).unwrap_or_default();
    if all.contains(advice) {
        return;
    }
    if !all.is_empty() {
        all.push_str("\n\n");
    }
    all.push_str(advice);
    if let Err(e) = std::fs::write(dir.join(ADVICE), all) {
        tracing::warn!(dir = %dir.display(), "review advice could not be kept: {e}");
    }
}

/// The advice no develop round has acted on yet.
pub fn pending_advice(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(ADVICE)).ok()
}

/// A develop round submitted: it has had the advice.
pub fn advice_given(dir: &Path) {
    let _ = std::fs::remove_file(dir.join(ADVICE));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minor_findings_are_kept_once_and_blocking_ones_are_not() {
        let d = tempfile::tempdir().unwrap();
        let f = json!([
            {"severity": "blocking", "issue": "broken"},
            {"severity": "minor", "issue": "a nit", "file": "a.rs"},
        ]);
        keep(d.path(), 3, &f);
        keep(d.path(), 5, &f);
        assert_eq!(lines(d.path()), vec!["minor: a nit (a.rs)"]);
    }

    #[test]
    fn feedback_marks_each_finding_and_only_minor_ones_are_advice() {
        let f = json!([
            {"severity": "minor", "issue": "a nit"},
            {"severity": "blocking", "issue": "broken", "file": "b.rs"},
        ]);
        let s = changes_feedback("no", &f);
        let (blocking, minor) = s.split_once("Minor findings").unwrap();
        assert!(blocking.contains("- [blocking] broken (b.rs)"), "{s}");
        assert!(minor.contains("- [minor] a nit"), "{s}");
        let a = advice(&f).unwrap();
        assert!(a.contains("[minor] a nit") && !a.contains("broken"), "{a}");
        assert_eq!(advice(&json!([])), None);
    }
}
