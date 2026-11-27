// SPDX-License-Identifier: GPL-3.0-or-later

//! What each step run said it tried and gave up (task 1128): one file per run,
//! `tried-r<run>.json` in the task's folder, written as the call that carries the list is
//! accepted, so a round that then fails leaves its notes behind. The words are the agent's;
//! `prompts::tried_block` fences them as data before another agent reads them.

use serde_json::{json, Value};
use std::path::Path;

/// One approach an agent gave up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tried {
    pub approach: String,
    pub why_dropped: String,
}

/// The list of one step run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Round {
    pub run: u64,
    pub step: String,
    pub tried: Vec<Tried>,
}

impl Round {
    /// As it is kept, and as `GET /tasks/{id}` gives it.
    pub fn to_json(&self) -> Value {
        json!({"run": self.run, "step": self.step, "tried": self.tried.iter().map(|t| {
            json!({"approach": t.approach, "why_dropped": t.why_dropped})
        }).collect::<Vec<_>>()})
    }

    fn from_json(v: &Value) -> Option<Round> {
        Some(Round {
            run: v["run"].as_u64()?,
            step: v["step"].as_str()?.to_string(),
            tried: entries(&v["tried"]),
        })
    }
}

fn entries(list: &Value) -> Vec<Tried> {
    list.as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| {
            Some(Tried {
                approach: e["approach"].as_str()?.to_string(),
                why_dropped: e["why_dropped"].as_str()?.to_string(),
            })
        })
        .collect()
}

fn file(run: u64) -> String {
    format!("tried-r{run}.json")
}

/// Keep `list` (an accepted call's `tried`) as the run's list, replacing an earlier one. A
/// list with no readable entry changes nothing.
pub fn keep(dir: &Path, run: u64, step: &str, list: &Value) {
    let tried = entries(list);
    if tried.is_empty() {
        return;
    }
    let round = Round {
        run,
        step: step.to_string(),
        tried,
    };
    let path = dir.join(file(run));
    let tmp = path.with_extension("tmp");
    let written = std::fs::write(&tmp, round.to_json().to_string())
        .and_then(|()| std::fs::rename(&tmp, &path));
    if let Err(e) = written {
        tracing::warn!(run, "the `tried` notes could not be kept: {e}");
    }
}

/// Every run of a task that left a list, newest first.
pub fn rounds(dir: &Path) -> Vec<Round> {
    let mut rounds: Vec<Round> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with("tried-r") && name.ends_with(".json")
        })
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| serde_json::from_str::<Value>(&t).ok())
        .filter_map(|v| Round::from_json(&v))
        .collect();
    rounds.sort_by_key(|r| std::cmp::Reverse(r.run));
    rounds
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(approach: &str) -> Value {
        json!([{"approach": approach, "why_dropped": "it did not work"}])
    }

    #[test]
    fn each_run_keeps_its_latest_list_and_rounds_read_newest_first() {
        let t = tempfile::tempdir().unwrap();
        keep(t.path(), 2, "develop", &one("first"));
        keep(t.path(), 10, "develop", &one("early"));
        keep(t.path(), 10, "develop", &one("late"));
        // Neither an empty list nor a bad one wipes what is there.
        keep(t.path(), 10, "develop", &json!([]));
        keep(t.path(), 10, "develop", &json!("all"));
        std::fs::write(t.path().join("change.json"), "{}").unwrap();
        let got: Vec<(u64, String)> = rounds(t.path())
            .into_iter()
            .map(|r| (r.run, r.tried[0].approach.clone()))
            .collect();
        assert_eq!(got, [(10, "late".to_string()), (2, "first".to_string())]);
        assert!(rounds(&t.path().join("none")).is_empty());
    }
}
