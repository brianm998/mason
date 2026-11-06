// SPDX-License-Identifier: GPL-3.0-or-later

//! The decision card (Part 10). In past records the median question ran 3,440
//! characters with the decisive line a fortieth of that, and over a third of
//! all question text was pasted logs. So `ask` takes only this shape:
//!
//! ```text
//! Ask:      one sentence, at most 200 characters
//! Why you:  why the ticket, repo and notes cannot settle it
//! Options:  2 to 4 complete answers, exactly one recommended
//! Evidence: at most 10 lines, plus links to stored logs
//! ```
//!
//! A card that does not fit is rewritten (by a cheap rewriter if one is
//! configured), and if that fails too, by a deterministic fallback, so no
//! question is ever lost. Answers are tied to [`hash`] of the card.

use serde_json::{json, Value};

pub const ASK_MAX: usize = 200;
pub const WHY_MAX: usize = 400;
pub const OPTION_MAX: usize = 160;
pub const EVIDENCE_LINES: usize = 10;
pub const EVIDENCE_LINE_MAX: usize = 300;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardOption {
    pub label: String,
    pub recommended: bool,
}

/// Where the card on the owner's screen came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Exactly as the agent wrote it.
    Agent,
    /// The agent's card did not fit and a rewriter produced a good one.
    Rewritten,
    /// Nothing else worked; built deterministically from what was given.
    Fallback,
}

impl Origin {
    pub fn name(self) -> &'static str {
        match self {
            Origin::Agent => "agent",
            Origin::Rewritten => "rewritten",
            Origin::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub ask: String,
    pub why: String,
    pub options: Vec<CardOption>,
    pub evidence: Vec<String>,
    pub links: Vec<String>,
    /// What this question is about, such as a gate's name. A second question
    /// with the same key after an answer is acknowledged, not asked again.
    pub key: String,
    pub origin: Origin,
    /// What was wrong with the agent's card, if it was replaced.
    pub problems: Vec<String>,
}

impl Card {
    /// The canonical JSON form: what is stored, shown and hashed.
    pub fn to_json(&self) -> Value {
        json!({
            "ask": self.ask,
            "why": self.why,
            "options": self.options.iter().map(|o| json!({"label": o.label, "recommended": o.recommended})).collect::<Vec<_>>(),
            "evidence": self.evidence,
            "links": self.links,
            "key": self.key,
            "origin": self.origin.name(),
            "problems": self.problems,
        })
    }

    /// Read a stored card back. `None` for text that is not one (older logs
    /// stored the bare question).
    pub fn from_json(v: &Value) -> Option<Card> {
        let s = |k: &str| v.get(k).and_then(Value::as_str).map(String::from);
        let list = |k: &str| -> Vec<String> {
            v.get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        Some(Card {
            ask: s("ask")?,
            why: s("why").unwrap_or_default(),
            options: v
                .get("options")?
                .as_array()?
                .iter()
                .filter_map(|o| {
                    Some(CardOption {
                        label: o.get("label")?.as_str()?.to_string(),
                        recommended: o.get("recommended").and_then(Value::as_bool) == Some(true),
                    })
                })
                .collect(),
            evidence: list("evidence"),
            links: list("links"),
            key: s("key").unwrap_or_default(),
            origin: match s("origin").as_deref() {
                Some("rewritten") => Origin::Rewritten,
                Some("fallback") => Origin::Fallback,
                _ => Origin::Agent,
            },
            problems: list("problems"),
        })
    }

    pub fn to_text(&self) -> String {
        self.to_json().to_string()
    }

    /// The option an answer of `n` (1-based) or a label picks, if any.
    pub fn pick(&self, answer: &str) -> Option<&CardOption> {
        let a = answer.trim();
        if let Ok(n) = a.parse::<usize>() {
            return self.options.get(n.checked_sub(1)?);
        }
        self.options
            .iter()
            .find(|o| o.label.eq_ignore_ascii_case(a))
    }
}

/// A stable identity for a card's text (FNV-1a, 16 hex characters). A late tap
/// on a stale card carries the old hash and is refused.
pub fn hash(text: &str) -> String {
    let h = text.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{h:016x}")
}

fn sentences(s: &str) -> usize {
    let chars: Vec<char> = s.trim().chars().collect();
    let mut n = 1;
    for i in 0..chars.len().saturating_sub(2) {
        if matches!(chars[i], '.' | '?' | '!')
            && chars[i + 1].is_whitespace()
            && chars[i + 2..].iter().any(|c| c.is_alphanumeric())
        {
            n += 1;
        }
    }
    n
}

fn evidence_lines(v: Option<&Value>, p: &mut Vec<String>) -> Vec<String> {
    let lines: Vec<String> = match v {
        None | Some(Value::Null) => vec![],
        Some(Value::String(s)) => s.lines().map(String::from).collect(),
        Some(Value::Array(a)) => {
            let mut out = vec![];
            for (i, x) in a.iter().enumerate() {
                match x.as_str() {
                    Some(s) => out.extend(s.lines().map(String::from)),
                    None => p.push(format!("evidence[{i}]: must be a string")),
                }
            }
            out
        }
        Some(_) => {
            p.push("evidence: must be text or a list of lines".into());
            vec![]
        }
    };
    if lines.len() > EVIDENCE_LINES {
        p.push(format!(
            "evidence: {} lines, the limit is {EVIDENCE_LINES}; quote the decisive lines and link the rest",
            lines.len()
        ));
    }
    if let Some(l) = lines.iter().find(|l| l.chars().count() > EVIDENCE_LINE_MAX) {
        p.push(format!(
            "evidence: a line is {} characters, the limit is {EVIDENCE_LINE_MAX}",
            l.chars().count()
        ));
    }
    lines
}

/// Read and check a card exactly as the agent wrote it. `Err` lists every
/// way it does not fit.
pub fn parse(a: &Value) -> Result<Card, Vec<String>> {
    let mut p = vec![];
    if !a.is_object() {
        return Err(vec!["arguments must be a JSON object".into()]);
    }
    let text = |key: &str, max: usize, p: &mut Vec<String>| -> String {
        match a.get(key) {
            None => {
                p.push(format!("{key}: required"));
                String::new()
            }
            Some(Value::String(s)) if s.trim().is_empty() => {
                p.push(format!("{key}: must not be empty"));
                String::new()
            }
            Some(Value::String(s)) if s.chars().count() > max => {
                p.push(format!(
                    "{key}: {} characters, the limit is {max}",
                    s.chars().count()
                ));
                s.clone()
            }
            Some(Value::String(s)) => s.clone(),
            Some(_) => {
                p.push(format!("{key}: must be a string"));
                String::new()
            }
        }
    };
    let ask = text("ask", ASK_MAX, &mut p);
    if ask.contains('\n') {
        p.push("ask: must be one line".into());
    } else if sentences(&ask) > 1 {
        p.push("ask: must be one sentence; put the rest in why or evidence".into());
    }
    let why = text("why", WHY_MAX, &mut p);
    let mut options = vec![];
    match a.get("options").and_then(Value::as_array) {
        Some(o) if (2..=4).contains(&o.len()) => {
            for (i, x) in o.iter().enumerate() {
                let label = x
                    .get("label")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim();
                if label.is_empty() {
                    p.push(format!("options[{i}].label: required"));
                } else if label.chars().count() > OPTION_MAX {
                    p.push(format!(
                        "options[{i}].label: longer than {OPTION_MAX} characters"
                    ));
                } else if label.split_whitespace().count() < 2 && label.chars().count() < 12 {
                    p.push(format!(
                        "options[{i}].label: {label:?} is not a complete answer; say what would happen"
                    ));
                }
                options.push(CardOption {
                    label: label.to_string(),
                    recommended: x.get("recommended").and_then(Value::as_bool) == Some(true),
                });
            }
            let rec = options.iter().filter(|o| o.recommended).count();
            if rec != 1 {
                p.push(format!(
                    "options: exactly one must be recommended (`recommended: true`), found {rec}"
                ));
            }
        }
        Some(o) => p.push(format!("options: need 2 to 4, found {}", o.len())),
        None => p.push("options: required, 2 to 4 complete answers".into()),
    }
    let evidence = evidence_lines(a.get("evidence"), &mut p);
    let links = match a.get("links") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(l)) => l
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
        Some(_) => {
            p.push("links: must be a list of strings".into());
            vec![]
        }
    };
    let key = a
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if key.chars().count() > 80 {
        p.push("key: longer than 80 characters".into());
    }
    if p.is_empty() {
        Ok(Card {
            ask,
            why,
            options,
            evidence,
            links,
            key,
            origin: Origin::Agent,
            problems: vec![],
        })
    } else {
        Err(p)
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

/// Build a card that always fits, from whatever the agent gave. Nothing is
/// thrown away: what does not fit moves to the evidence, and the problems are
/// recorded on the card.
pub fn fallback(a: &Value, problems: &[String]) -> Card {
    let raw_ask = a
        .get("ask")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let first_line = raw_ask.lines().next().unwrap_or("").trim().to_string();
    let first_sentence: String = {
        let chars: Vec<char> = first_line.chars().collect();
        let mut end = chars.len();
        for i in 0..chars.len().saturating_sub(2) {
            if matches!(chars[i], '.' | '?' | '!') && chars[i + 1].is_whitespace() {
                end = i + 1;
                break;
            }
        }
        chars[..end].iter().collect()
    };
    let ask = if first_sentence.is_empty() {
        "The agent needs a decision it could not make itself".to_string()
    } else {
        truncate(&first_sentence, ASK_MAX)
    };
    let why = a
        .get("why")
        .and_then(Value::as_str)
        .map(|w| truncate(w.lines().next().unwrap_or("").trim(), WHY_MAX))
        .filter(|w| !w.is_empty())
        .unwrap_or_else(|| {
            "The agent could not settle this from the ticket, the repository and the notes.".into()
        });

    let mut options: Vec<CardOption> = vec![];
    if let Some(list) = a.get("options").and_then(Value::as_array) {
        for x in list {
            let label = x
                .get("label")
                .and_then(Value::as_str)
                .or_else(|| x.as_str())
                .unwrap_or("")
                .trim();
            if !label.is_empty() && options.len() < 4 {
                // A bare "a" is not an answer a person can pick with confidence.
                let label = if label.split_whitespace().count() < 2 && label.chars().count() < 12 {
                    format!("Choose: {label}")
                } else {
                    label.to_string()
                };
                options.push(CardOption {
                    label: truncate(&label, OPTION_MAX),
                    recommended: x.get("recommended").and_then(Value::as_bool) == Some(true),
                });
            }
        }
    }
    while options.len() < 2 {
        let label = if options.is_empty() {
            "Go ahead with the agent's best judgment"
        } else {
            "Stop and wait for further instructions"
        };
        options.push(CardOption {
            label: label.into(),
            recommended: false,
        });
    }
    // Exactly one recommended: the first the agent marked, else the first.
    let pick = options.iter().position(|o| o.recommended).unwrap_or(0);
    for (i, o) in options.iter_mut().enumerate() {
        o.recommended = i == pick;
    }

    // Whatever did not fit goes to the evidence, trimmed to the limit.
    let mut p = vec![];
    let mut lines = evidence_lines_lenient(a.get("evidence"));
    let rest: Vec<&str> = raw_ask
        .lines()
        .skip(1)
        .chain(std::iter::once(
            raw_ask.strip_prefix(&first_sentence).unwrap_or("").trim(),
        ))
        .filter(|l| !l.trim().is_empty() && !first_sentence.contains(l.trim()))
        .collect();
    for l in rest {
        lines.push(l.to_string());
    }
    let dropped = lines.len().saturating_sub(EVIDENCE_LINES);
    lines.truncate(EVIDENCE_LINES);
    let evidence: Vec<String> = lines
        .iter()
        .map(|l| truncate(l, EVIDENCE_LINE_MAX))
        .collect();
    if dropped > 0 {
        p.push(format!("{dropped} more line(s) of evidence were dropped"));
    }
    let mut probs: Vec<String> = problems.to_vec();
    probs.extend(p);
    Card {
        ask,
        why,
        options,
        evidence,
        links: a
            .get("links")
            .and_then(Value::as_array)
            .map(|l| {
                l.iter()
                    .filter_map(|x| x.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default(),
        key: truncate(a.get("key").and_then(Value::as_str).unwrap_or(""), 80),
        origin: Origin::Fallback,
        problems: probs,
    }
}

fn evidence_lines_lenient(v: Option<&Value>) -> Vec<String> {
    match v {
        Some(Value::String(s)) => s.lines().map(String::from).collect(),
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .flat_map(|s| s.lines().map(String::from).collect::<Vec<_>>())
            .collect(),
        _ => vec![],
    }
}

/// A card that came back from a rewriter must itself fit; if it does, mark its
/// origin and record what was wrong with the original.
pub fn accept_rewrite(rewritten: &Value, problems: &[String]) -> Option<Card> {
    let mut c = parse(rewritten).ok()?;
    c.origin = Origin::Rewritten;
    c.problems = problems.to_vec();
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn good() -> Value {
        json!({
            "ask": "Should the token expire after one hour or one day?",
            "why": "The ticket does not say and the security notes are silent.",
            "options": [
                {"label": "One hour, with refresh tokens", "recommended": true},
                {"label": "One day, no refresh tokens"}
            ],
            "evidence": ["ticket says only 'tokens expire'", "no mention in SECURITY.md"]
        })
    }

    #[test]
    fn a_card_that_fits_is_accepted_as_written() {
        let c = parse(&good()).unwrap();
        assert_eq!(c.origin, Origin::Agent);
        assert_eq!(c.options.len(), 2);
        assert_eq!(Card::from_json(&c.to_json()).unwrap(), c);
    }

    fn problems(mutate: impl FnOnce(&mut Value)) -> Vec<String> {
        let mut v = good();
        mutate(&mut v);
        parse(&v).unwrap_err()
    }

    #[test]
    fn every_rule_of_the_shape_is_enforced() {
        let has =
            |p: Vec<String>, s: &str| assert!(p.iter().any(|m| m.contains(s)), "{s} in {p:?}");
        has(
            problems(|v| v["ask"] = json!("x".repeat(201))),
            "the limit is 200",
        );
        has(
            problems(|v| v["ask"] = json!("First. Second thing here.")),
            "one sentence",
        );
        has(
            problems(|v| v["ask"] = json!("line one\nline two")),
            "one line",
        );
        has(
            problems(|v| {
                v.as_object_mut().unwrap().remove("why");
            }),
            "why: required",
        );
        has(problems(|v| v["why"] = json!("y".repeat(401))), "why:");
        has(
            problems(|v| {
                v["options"] = json!([{"label": "Only one choice here", "recommended": true}])
            }),
            "need 2 to 4",
        );
        has(
            problems(|v| {
                v["options"] = json!([1, 2, 3, 4, 5]
                    .map(|i| json!({"label": format!("option number {i}"), "recommended": i == 1})))
            }),
            "need 2 to 4",
        );
        has(
            problems(|v| {
                v["options"] = json!([{"label": "Use one hour"}, {"label": "Use one day"}])
            }),
            "exactly one must be recommended",
        );
        has(
            problems(
                |v| v["options"] = json!([{"label": "Use one hour", "recommended": true}, {"label": "Use one day", "recommended": true}]),
            ),
            "found 2",
        );
        has(
            problems(|v| {
                v["options"] = json!([{"label": "a", "recommended": true}, {"label": "b"}])
            }),
            "not a complete answer",
        );
        has(
            problems(|v| {
                v["evidence"] = json!((0..11).map(|i| format!("line {i}")).collect::<Vec<_>>())
            }),
            "11 lines",
        );
        has(
            problems(|v| v["evidence"] = json!("z".repeat(301))),
            "a line is 301",
        );
        has(problems(|v| v["key"] = json!("k".repeat(81))), "key:");
        assert!(parse(&json!("not an object")).is_err());
    }

    #[test]
    fn the_fallback_always_fits_and_loses_nothing() {
        // A pasted log as the question, one-word options, nothing else.
        let log: String = (0..40)
            .map(|i| format!("error line {i}: something broke"))
            .collect::<Vec<_>>()
            .join("\n");
        let bad = json!({"ask": format!("Why does the build fail?\n{log}"), "options": [{"label": "a"}, {"label": "b"}]});
        let problems = parse(&bad).unwrap_err();
        let c = fallback(&bad, &problems);
        assert_eq!(c.origin, Origin::Fallback);
        assert_eq!(c.ask, "Why does the build fail?");
        assert!(
            parse(&c.to_json()).is_ok(),
            "the fallback is itself a valid card: {c:?}"
        );
        assert_eq!(c.evidence.len(), 10);
        assert!(c
            .problems
            .iter()
            .any(|p| p.contains("more line(s) of evidence were dropped")));
        assert!(c
            .problems
            .iter()
            .any(|p| p.contains("not a complete answer")));
        assert_eq!(c.options.iter().filter(|o| o.recommended).count(), 1);
    }

    #[test]
    fn the_fallback_copes_with_nothing_at_all() {
        for bad in [
            json!({}),
            json!({"ask": ""}),
            json!({"options": "nope"}),
            json!({"ask": 7}),
        ] {
            let c = fallback(&bad, &["bad".into()]);
            assert!(
                c.options.len() >= 2 && c.options.iter().filter(|o| o.recommended).count() == 1
            );
            assert!(c.ask.chars().count() <= ASK_MAX && !c.ask.is_empty());
            // Fits, except that a fallback may carry options that need no review.
            let again = Card::from_json(&c.to_json()).unwrap();
            assert_eq!(again, c);
        }
    }

    #[test]
    fn a_long_single_sentence_is_cut_not_lost() {
        let long = format!("Should we {} or not?", "really ".repeat(60));
        let c = fallback(
            &json!({"ask": long, "options": [{"label": "Yes, do it", "recommended": true}, {"label": "No, leave it"}]}),
            &[],
        );
        assert!(c.ask.chars().count() <= ASK_MAX && c.ask.ends_with('…'));
    }

    #[test]
    fn a_rewrite_must_itself_fit() {
        assert_eq!(
            accept_rewrite(&good(), &["was long".into()])
                .unwrap()
                .origin,
            Origin::Rewritten
        );
        assert!(accept_rewrite(&json!({"ask": "still bad"}), &[]).is_none());
    }

    #[test]
    fn hashes_follow_the_text_and_answers_pick_options() {
        let c = parse(&good()).unwrap();
        let h = hash(&c.to_text());
        assert_eq!(h.len(), 16);
        assert_eq!(h, hash(&c.to_text()));
        let mut other = c.clone();
        other.ask.push('!');
        assert_ne!(h, hash(&other.to_text()));
        assert_eq!(c.pick("1").unwrap().label, "One hour, with refresh tokens");
        assert_eq!(
            c.pick("one day, no refresh tokens").unwrap().label,
            "One day, no refresh tokens"
        );
        assert!(c.pick("3").is_none() && c.pick("0").is_none() && c.pick("whatever").is_none());
    }
}
