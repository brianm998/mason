// SPDX-License-Identifier: GPL-3.0-or-later

//! A task's plan as readable parts. Pure: the text of `plan.json` in, a [`PlanView`] out.
//! The monitors draw the parts and do no JSON work.

use mason_protocol::{PlanExtra, PlanView};
use serde_json::Value;

/// Shown when the file is not a JSON object.
pub const UNREADABLE_NOTE: &str = "This plan could not be read as a plan; showing it as written.";

const KNOWN: [&str; 4] = ["summary", "files", "approach", "acceptance_criteria"];

/// Parse the text of a plan. Anything that is not a JSON object yields `raw` and the note only.
pub fn parse_plan(raw: &str) -> PlanView {
    let obj = match serde_json::from_str::<Value>(raw) {
        Ok(Value::Object(o)) => o,
        _ => {
            return PlanView {
                raw: raw.to_string(),
                note: UNREADABLE_NOTE.to_string(),
                ..Default::default()
            }
        }
    };
    let mut v = PlanView {
        raw: raw.to_string(),
        ..Default::default()
    };
    if let Some(Value::String(s)) = obj.get("summary") {
        v.summary_paragraphs = paragraphs(s);
    }
    if let Some(Value::String(a)) = obj.get("approach") {
        let (lead, items, numbers, is_list) = split_approach(a);
        v.lead = lead;
        v.steps = items;
        v.step_numbers = numbers;
        v.steps_are_list = is_list;
    }
    v.files = strings(obj.get("files"));
    v.criteria = strings(obj.get("acceptance_criteria"));
    for (k, val) in &obj {
        if KNOWN.contains(&k.as_str()) {
            continue;
        }
        let value = readable(val, 0);
        v.other.push(PlanExtra {
            label: label(k),
            value,
        });
    }
    v
}

/// "summary_placeholder" -> "Summary placeholder".
pub fn label(key: &str) -> String {
    let s = key.replace('_', " ");
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                other => readable(other, 0),
            })
            .filter(|s| !s.trim().is_empty())
            .collect(),
        Some(Value::String(s)) if !s.trim().is_empty() => vec![s.clone()],
        _ => Vec::new(),
    }
}

fn paragraphs(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    for line in s.lines() {
        if line.trim().is_empty() {
            if !cur.is_empty() {
                out.push(cur.join("\n"));
                cur.clear();
            }
        } else {
            cur.push(line);
        }
    }
    if !cur.is_empty() {
        out.push(cur.join("\n"));
    }
    out
}

/// A list marker at the start of a line: (the number as written or "" for a bullet, the rest).
fn marker(line: &str) -> Option<(String, &str)> {
    let t = line.trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        let rest = &t[digits..];
        let mut it = rest.chars();
        if matches!(it.next(), Some('.' | ')')) {
            let after = it.as_str();
            if after.starts_with(char::is_whitespace) {
                return Some((t[..digits].to_string(), after.trim_start()));
            }
        }
        return None;
    }
    let mut it = t.chars();
    if matches!(it.next(), Some('-' | '*')) {
        let after = it.as_str();
        if after.starts_with(char::is_whitespace) {
            return Some((String::new(), after.trim_start()));
        }
    }
    None
}

/// (lead, items, numbers, is_list)
fn split_approach(a: &str) -> (String, Vec<String>, Vec<String>, bool) {
    let mut lead: Vec<&str> = Vec::new();
    let mut items: Vec<Vec<String>> = Vec::new();
    let mut numbers: Vec<String> = Vec::new();
    for line in a.lines() {
        if let Some((n, rest)) = marker(line) {
            numbers.push(n);
            items.push(vec![rest.trim_end().to_string()]);
        } else if let Some(last) = items.last_mut() {
            last.push(line.trim().to_string());
        } else {
            lead.push(line);
        }
    }
    if items.is_empty() {
        return (String::new(), paragraphs(a), Vec::new(), false);
    }
    let items: Vec<String> = items
        .into_iter()
        .map(|l| l.join("\n").trim().to_string())
        .collect();
    let numbered = numbers.iter().all(|n| !n.is_empty());
    let numbers = if numbered { numbers } else { Vec::new() };
    (lead.join("\n").trim().to_string(), items, numbers, true)
}

/// A value as readable text, no braces or quotes.
fn readable(v: &Value, depth: usize) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => {
                    format!("- {}", readable(x, depth))
                }
                _ => format!("- {}", readable(x, depth + 1).trim_start()),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Object(o) => o
            .iter()
            .map(|(k, x)| {
                let pad = "  ".repeat(depth + 1);
                match x {
                    Value::Object(_) | Value::Array(_) => {
                        let inner = readable(x, depth + 1);
                        format!("{pad}{k}:\n{inner}")
                    }
                    _ => format!("{pad}{k}: {}", readable(x, depth + 1)),
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: Value) -> PlanView {
        parse_plan(&v.to_string())
    }

    fn approach(a: &str) -> PlanView {
        parse(json!({ "approach": a }))
    }

    #[test]
    fn full_plan() {
        let v = parse(json!({
            "summary": "First para.\n\nSecond `code` para.",
            "files": ["a.rs", "b/c.rs"],
            "approach": "1. do a\n2. do b",
            "acceptance_criteria": ["x works", "y works"],
        }));
        assert_eq!(
            v.summary_paragraphs,
            vec!["First para.", "Second `code` para."]
        );
        assert_eq!(v.files, vec!["a.rs", "b/c.rs"]);
        assert_eq!(v.steps, vec!["do a", "do b"]);
        assert_eq!(v.step_numbers, vec!["1", "2"]);
        assert!(v.steps_are_list);
        assert_eq!(v.criteria, vec!["x works", "y works"]);
        assert!(v.other.is_empty() && v.note.is_empty());
        assert!(v.raw.starts_with('{'));
    }

    #[test]
    fn numbered_from_zero_and_one() {
        let z = approach("0. a\n1. b\n2. c");
        assert_eq!(z.step_numbers, vec!["0", "1", "2"]);
        assert_eq!(z.steps, vec!["a", "b", "c"]);
        let o = approach("1. a\n2. b");
        assert_eq!(o.step_numbers, vec!["1", "2"]);
    }

    #[test]
    fn paren_markers() {
        let v = approach("1) a\n2) b");
        assert_eq!(v.steps, vec!["a", "b"]);
        assert_eq!(v.step_numbers, vec!["1", "2"]);
    }

    #[test]
    fn bullets() {
        for a in ["- a\n- b", "* a\n* b", "- a\n* b"] {
            let v = approach(a);
            assert!(v.steps_are_list);
            assert_eq!(v.steps, vec!["a", "b"]);
            assert!(v.step_numbers.is_empty());
        }
    }

    #[test]
    fn multi_line_steps_and_lead() {
        let v = approach("Intro line\n1. first\n   more of first\n   and more\n2. second");
        assert_eq!(v.lead, "Intro line");
        assert_eq!(v.steps, vec!["first\nmore of first\nand more", "second"]);
    }

    #[test]
    fn not_a_list() {
        let v = approach("Just do it.\n\nThen test it. 3.5 is fine.\n-not a bullet");
        assert!(!v.steps_are_list);
        assert_eq!(v.steps.len(), 2);
        assert!(v.step_numbers.is_empty());
        assert!(v.lead.is_empty());
    }

    #[test]
    fn missing_field_leaves_part_empty() {
        let v = parse(json!({ "summary": "only this" }));
        assert_eq!(v.summary_paragraphs, vec!["only this"]);
        assert!(v.steps.is_empty() && v.files.is_empty() && v.criteria.is_empty());
        assert!(v.note.is_empty());
    }

    #[test]
    fn extra_string_key() {
        let v = parse(json!({ "summary": "s", "summary_placeholder": "later" }));
        assert_eq!(v.other.len(), 1);
        assert_eq!(v.other[0].label, "Summary placeholder");
        assert_eq!(v.other[0].value, "later");
    }

    #[test]
    fn extra_list_and_object_keys() {
        let v = parse(json!({
            "risks": ["a", "b"],
            "meta": { "owner": "me", "deep": { "n": 3 } },
        }));
        let risks = v.other.iter().find(|e| e.label == "Risks").unwrap();
        assert_eq!(risks.value, "- a\n- b");
        let meta = v.other.iter().find(|e| e.label == "Meta").unwrap();
        for bad in ['{', '}', '"'] {
            assert!(!meta.value.contains(bad), "{}", meta.value);
        }
        assert!(meta.value.contains("  owner: me"));
        assert!(meta.value.contains("n: 3"));
    }

    #[test]
    fn invalid_json_and_array() {
        for raw in ["{\"summary\": ", "[1,2]", "", "\"str\""] {
            let v = parse_plan(raw);
            assert_eq!(v.raw, raw);
            assert_eq!(v.note, UNREADABLE_NOTE);
            assert!(v.summary_paragraphs.is_empty() && v.steps.is_empty());
            assert!(v.files.is_empty() && v.criteria.is_empty() && v.other.is_empty());
        }
    }

    #[test]
    fn real_size_fixture() {
        let steps: Vec<String> = (0..12)
            .map(|i| {
                format!(
                    "{i}. Step {i}: change `file{i}.rs` and **check** it; {}",
                    "x".repeat(250)
                )
            })
            .collect();
        let approach = steps.join("\n");
        assert!(approach.len() > 3_500);
        let files: Vec<String> = (0..8).map(|i| format!("crates/x/src/f{i}.rs")).collect();
        let crit: Vec<String> = (0..13).map(|i| format!("criterion {i}")).collect();
        let v = parse(json!({
            "summary": "s", "files": files, "approach": approach, "acceptance_criteria": crit,
        }));
        assert_eq!(v.files.len(), 8);
        assert_eq!(v.criteria.len(), 13);
        assert_eq!(v.steps.len(), 12);
        assert_eq!(v.step_numbers[0], "0");
        assert!(v.steps.iter().all(|s| !s.contains("\\n")));
    }
}
