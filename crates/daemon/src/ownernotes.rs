// SPDX-License-Identifier: GPL-3.0-or-later

//! The notes the owner gave a task's retries (task 988). A retry note is shown once to the next
//! agent step (`jobs::take_retry_note`); the owner's are also kept here, per task, so the
//! reviewer and later develop rounds see the decisions too. They go when the task lands or is
//! canceled, or when the owner clears them.

use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};

/// One JSON object per line: `{"at_ms", "step", "note"}`.
pub const FILE: &str = "owner-notes";

fn path(dir: &Path) -> PathBuf {
    dir.join(FILE)
}

/// Append an owner's note, with the time and the step the retry started from.
pub fn append(dir: &Path, at_ms: u64, step: &str, note: &str) {
    let note = note.trim();
    if note.is_empty() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let line = json!({"at_ms": at_ms, "step": step, "note": note}).to_string();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path(dir))
    {
        let _ = writeln!(f, "{line}");
    }
}

/// Every kept note, oldest first.
pub fn list(dir: &Path) -> Vec<Value> {
    std::fs::read_to_string(path(dir))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .filter(|v| v["note"].as_str().is_some_and(|n| !n.trim().is_empty()))
        .collect()
}

/// Remove them all. Says how many went.
pub fn clear(dir: &Path) -> usize {
    let n = list(dir).len();
    let _ = std::fs::remove_file(path(dir));
    n
}

/// The prompt section for the notes in `dir`, if any.
pub fn block(dir: &Path) -> Option<String> {
    render(&list(dir))
}

pub fn render(notes: &[Value]) -> Option<String> {
    if notes.is_empty() {
        return None;
    }
    let mut s = String::from(
        "=== Owner decisions on this task ===\nThe owner gave these notes when retrying this task. They are the owner's decisions: where one conflicts with the task text above, the note wins (a reduced scope, an accepted deviation). Judge and work against them.\n",
    );
    for n in notes {
        s.push_str(&format!(
            "- (at the `{}` step) {}\n",
            n["step"].as_str().unwrap_or("?"),
            n["note"].as_str().unwrap_or("").trim()
        ));
    }
    s.push_str("=== end of owner decisions ===");
    Some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_are_kept_in_order_and_cleared() {
        let d = tempfile::tempdir().unwrap();
        assert!(block(d.path()).is_none());
        append(d.path(), 1, "develop", "scope is reduced");
        append(d.path(), 2, "review", "  ");
        append(d.path(), 3, "review", "deviation accepted");
        let l = list(d.path());
        assert_eq!(l.len(), 2);
        let b = block(d.path()).unwrap();
        assert!(b.contains("Owner decisions on this task"));
        assert!(b.contains("the note wins"));
        assert!(b.find("scope is reduced") < b.find("deviation accepted"));
        assert_eq!(clear(d.path()), 2);
        assert!(block(d.path()).is_none());
    }
}
