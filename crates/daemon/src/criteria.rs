// SPDX-License-Identifier: GPL-3.0-or-later

//! Acceptance criteria as data (task 1136). The plan's criteria each name a check; develop
//! reports one status per criterion in `submit_change`; review grades each in `submit_review`.
//! Pure: JSON in, text or JSON out. Old plans keep plain-string criteria, which read as a
//! criterion with no check.

use serde_json::{json, Value};

/// One plan criterion: its text and the check that verifies it (none in an old plan).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Criterion {
    pub criterion: String,
    pub check: Option<String>,
}

/// The criteria of a plan, in order, whichever shape it was written in.
pub fn of_plan(plan: &Value) -> Vec<Criterion> {
    plan["acceptance_criteria"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| match x {
                    Value::String(s) => Some(Criterion {
                        criterion: s.clone(),
                        check: None,
                    }),
                    Value::Object(o) => Some(Criterion {
                        criterion: o.get("criterion")?.as_str()?.to_string(),
                        check: o.get("check").and_then(Value::as_str).map(String::from),
                    }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether the plan's criteria carry checks, so develop must report on each.
pub fn is_graded(plan: &Value) -> bool {
    plan["acceptance_criteria"]
        .as_array()
        .is_some_and(|a| a.iter().any(Value::is_object))
}

/// What is wrong with a `submit_change` that must report on `plan`'s criteria.
pub fn change_problems(plan: &Value, args: &Value) -> Vec<String> {
    if !is_graded(plan) {
        return vec![];
    }
    let want = of_plan(plan).len();
    match args.get("criteria").and_then(Value::as_array) {
        None => vec![format!(
            "criteria: required, one entry per plan criterion in order ({want}): {{status: met | not_met | partly, evidence}}"
        )],
        Some(a) if a.len() != want => vec![format!(
            "criteria: {} entries, the plan has {want} criteria; give one per criterion, in order",
            a.len()
        )],
        Some(_) => vec![],
    }
}

/// What is wrong with a `submit_review` that must grade `plan`'s criteria, given that develop
/// reported on them.
pub fn review_problems(plan: &Value, change: &Value, args: &Value) -> Vec<String> {
    if !is_graded(plan) || change.get("criteria").and_then(Value::as_array).is_none() {
        return vec![];
    }
    let want = of_plan(plan).len();
    match args.get("criteria").and_then(Value::as_array) {
        None => vec![format!(
            "criteria: required, one entry per plan criterion in order ({want}): {{status: agree | disagree, why}}"
        )],
        Some(a) if a.len() != want => vec![format!(
            "criteria: {} entries, the plan has {want} criteria; give one per criterion, in order",
            a.len()
        )],
        Some(_) => vec![],
    }
}

fn status(change: Option<&Value>, i: usize) -> Option<(String, String)> {
    let e = change?.get("criteria")?.as_array()?.get(i)?;
    Some((
        e["status"].as_str()?.to_string(),
        e["evidence"].as_str().unwrap_or("").to_string(),
    ))
}

/// One line per criterion: `1. criterion — check: …`, with develop's status when `change` has one.
pub fn lines(plan: &Value, change: Option<&Value>) -> Vec<String> {
    of_plan(plan)
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut s = c.criterion.clone();
            if let Some(k) = &c.check {
                s.push_str(&format!(" — check: {k}"));
            }
            if let Some((st, ev)) = status(change, i) {
                s.push_str(&format!(" — develop: {st}"));
                if !ev.is_empty() {
                    s.push_str(&format!(" ({ev})"));
                }
            }
            s
        })
        .collect()
}

/// The block a review or diagnosis prompt carries, or `None` when the plan has no criteria.
pub fn block(plan: Option<&Value>, change: Option<&Value>) -> Option<String> {
    let l = lines(plan?, change);
    if l.is_empty() {
        return None;
    }
    let numbered: Vec<String> = l
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}. {s}", i + 1))
        .collect();
    let mut out = format!(
        "Acceptance criteria and develop's report on each:\n{}",
        numbered.join("\n")
    );
    if change.is_some_and(|c| c.get("criteria").is_some()) {
        out.push_str(
            "\nGrade each criterion in `criteria` of submit_review, in this order: {status: agree | disagree, why}. \
             A `disagree` on a criterion develop marked `met` is a blocking finding.",
        );
    }
    Some(out)
}

/// An accepted review with its criteria folded in: each `disagree` on a criterion develop
/// marked `met` becomes a blocking finding, and an `approve` carrying one becomes `changes`.
pub fn apply_review(plan: Option<&Value>, change: Option<&Value>, submission: Value) -> Value {
    let (Some(plan), Some(change)) = (plan, change) else {
        return submission;
    };
    let list = of_plan(plan);
    let Some(grades) = submission
        .get("criteria")
        .and_then(Value::as_array)
        .cloned()
    else {
        return submission;
    };
    let mut extra = vec![];
    for (i, g) in grades.iter().enumerate() {
        let met = status(Some(change), i).is_some_and(|(s, _)| s == "met");
        if g["status"] == "disagree" && met {
            let Some(c) = list.get(i) else { continue };
            extra.push(json!({
                "issue": format!(
                    "Acceptance criterion {} was reported met by develop, but the reviewer disagrees: {}",
                    i + 1,
                    g["why"].as_str().unwrap_or("")
                ),
                "quote": c.criterion,
                "kind": "other",
                "severity": "blocking",
            }));
        }
    }
    if extra.is_empty() {
        return submission;
    }
    let mut s = submission;
    let mut f = s["findings"].as_array().cloned().unwrap_or_default();
    f.extend(extra);
    s["findings"] = Value::Array(f);
    s["verdict"] = json!("changes");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> Value {
        json!({"acceptance_criteria": [
            {"criterion": "a works", "check": "test a"},
            {"criterion": "b works", "check": "test b"}]})
    }

    #[test]
    fn old_string_plans_are_read_and_not_graded() {
        let old = json!({"acceptance_criteria": ["x", "y"]});
        assert_eq!(of_plan(&old).len(), 2);
        assert!(!is_graded(&old));
        assert!(change_problems(&old, &json!({})).is_empty());
        assert_eq!(lines(&old, None), vec!["x", "y"]);
    }

    #[test]
    fn change_must_report_one_entry_per_criterion() {
        assert!(!change_problems(&plan(), &json!({})).is_empty());
        let one = json!({"criteria": [{"status": "met", "evidence": "t"}]});
        let why = change_problems(&plan(), &one).join("\n");
        assert!(why.contains("1 entries, the plan has 2"), "{why}");
        let two = json!({"criteria": [{"status": "met", "evidence": "t"}, {"status": "met", "evidence": "t"}]});
        assert!(change_problems(&plan(), &two).is_empty());
    }

    #[test]
    fn disagreeing_with_a_met_criterion_is_blocking() {
        let change = json!({"criteria": [
            {"status": "met", "evidence": "test a passed"},
            {"status": "not_met", "evidence": "-"}]});
        let review = json!({"verdict": "approve", "summary": "s", "criteria": [
            {"status": "disagree", "why": "test a does not test it"},
            {"status": "disagree", "why": "agreed it is not met"}]});
        let out = apply_review(Some(&plan()), Some(&change), review.clone());
        assert_eq!(out["verdict"], "changes");
        let f = out["findings"].as_array().unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0]["severity"], "blocking");
        assert!(f[0]["issue"].as_str().unwrap().contains("criterion 1"));
        let agree = json!({"verdict": "approve", "summary": "s", "criteria": [
            {"status": "agree", "why": "ok"}, {"status": "disagree", "why": "x"}]});
        assert_eq!(
            apply_review(Some(&plan()), Some(&change), agree)["verdict"],
            "approve"
        );
    }

    #[test]
    fn the_block_shows_checks_and_statuses() {
        let change = json!({"criteria": [
            {"status": "met", "evidence": "test a passed"},
            {"status": "not_met", "evidence": "none"}]});
        let b = block(Some(&plan()), Some(&change)).unwrap();
        assert!(
            b.contains("1. a works — check: test a — develop: met (test a passed)"),
            "{b}"
        );
        assert!(
            b.contains("2. b works — check: test b — develop: not_met"),
            "{b}"
        );
        assert!(b.contains("blocking finding"));
    }
}

#[cfg(test)]
mod flow {
    use super::*;

    /// explore submits two criteria with checks; develop reports one met and one not_met with a
    /// note; review disagrees on the met one; the feedback names that criterion.
    #[test]
    fn scripted_explore_develop_review_sends_the_task_back_naming_the_criterion() {
        use mason_adapter_api::contract::validate;
        let plan = json!({"summary": "s", "files": ["a.rs"], "approach": "a",
            "acceptance_criteria": [
                {"criterion": "a works", "check": "test a"},
                {"criterion": "b works", "check": "test b"}]});
        validate("submit_plan", &plan).unwrap();
        let change = json!({"summary": "s", "head": "0123abc", "notes": "b needs a platform call",
            "criteria": [{"status": "met", "evidence": "test a passed"},
                         {"status": "not_met", "evidence": "none"}]});
        validate("submit_change", &change).unwrap();
        assert!(change_problems(&plan, &change).is_empty());
        let review = json!({"verdict": "approve", "summary": "s", "criteria": [
            {"status": "disagree", "why": "test a never calls the new code"},
            {"status": "agree", "why": "not met"}]});
        validate("submit_review", &review).unwrap();
        assert!(review_problems(&plan, &change, &review).is_empty());
        let out = apply_review(Some(&plan), Some(&change), review);
        assert_eq!(out["verdict"], "changes");
        let feedback = crate::reviewfindings::changes_feedback("s", &out["findings"]);
        assert!(feedback.contains("criterion 1"), "{feedback}");
        assert!(
            feedback.contains("test a never calls the new code"),
            "{feedback}"
        );
    }
}
