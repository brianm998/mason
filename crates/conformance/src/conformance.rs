// SPDX-License-Identifier: GPL-3.0-or-later

//! What a client must show. The server's views say exactly what is visible and
//! what can be done; a client's accessibility tree is read into the same list
//! of items; the two are compared. Both monitors must pass the same run
//! against `masond --demo`, so parity is a machine check (Part 17).

use mason_protocol::view::Body;
use mason_protocol::*;
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// One thing on screen: a role and an id (`tile:testing`, `row:7`), the texts
/// a person can read in it, and the actions it offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub texts: Vec<String>,
    pub actions: Vec<String>,
    /// Position among its siblings, where order is part of the rule.
    pub order: usize,
}

impl Item {
    fn new(id: impl Into<String>, texts: Vec<String>, actions: Vec<String>, order: usize) -> Item {
        let mut actions = actions;
        actions.sort();
        Item {
            id: id.into(),
            texts,
            actions,
            order,
        }
    }

    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "texts": self.texts, "actions": self.actions, "order": self.order})
    }

    pub fn from_json(v: &Value) -> Option<Item> {
        let list = |k: &str| -> Vec<String> {
            v[k].as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default()
        };
        let mut actions = list("actions");
        actions.sort();
        Some(Item {
            id: v["id"].as_str()?.to_string(),
            texts: list("texts"),
            actions,
            order: v["order"].as_u64().unwrap_or(0) as usize,
        })
    }
}

fn row_item(r: &TaskRow, order: usize, prefix: &str) -> Item {
    let mut texts = vec![
        r.key.clone(),
        r.title.clone(),
        r.project.clone(),
        r.state.clone(),
    ];
    if !r.why_waiting.is_empty() {
        texts.push(r.why_waiting.clone());
    }
    Item::new(
        format!("{prefix}:{}", r.id),
        texts,
        r.allowed_actions.clone(),
        order,
    )
}

fn card_item(task: u64, c: &DecisionCard) -> Item {
    let mut texts = vec![c.ask.clone(), c.why.clone()];
    for (i, o) in c.options.iter().enumerate() {
        texts.push(format!(
            "{}. {}{}",
            i + 1,
            o.label,
            if o.recommended { " (recommended)" } else { "" }
        ));
    }
    texts.extend(c.evidence.iter().cloned());
    // Pick an option, or type an answer: what the card offers.
    let mut actions: Vec<String> = (1..=c.options.len())
        .map(|i| format!("option:{i}"))
        .collect();
    actions.push("other".into());
    Item::new(format!("card:{task}"), texts, actions, 0)
}

/// Everything the views say is visible, as a flat list.
pub fn visible(views: &[View]) -> Vec<Item> {
    let mut out = vec![];
    for v in views {
        match v.body.as_ref() {
            Some(Body::Header(h)) => {
                out.push(Item::new("status", vec![h.status.clone()], vec![], 0));
                for (i, c) in h.counts.iter().enumerate() {
                    out.push(Item::new(
                        format!("tile:{}", c.state),
                        vec![c.count.to_string(), c.state.clone()],
                        vec![],
                        i,
                    ));
                }
                for (i, b) in h.banners.iter().enumerate() {
                    out.push(Item::new(
                        format!("banner:{i}"),
                        vec![b.text.clone()],
                        vec![],
                        i,
                    ));
                }
            }
            Some(Body::Sidebar(s)) => {
                for (i, sec) in s.sections.iter().enumerate() {
                    out.push(Item::new(
                        format!("section:{}", sec.state),
                        vec![
                            sec.title.clone(),
                            sec.count.to_string(),
                            if sec.collapsed {
                                "collapsed"
                            } else {
                                "expanded"
                            }
                            .to_string(),
                        ],
                        vec![],
                        i,
                    ));
                    // A collapsed section shows no rows.
                    if !sec.collapsed {
                        for (j, r) in sec.rows.iter().enumerate() {
                            out.push(row_item(r, j, &format!("row:{}", sec.state)));
                        }
                    }
                }
            }
            Some(Body::Home(h)) => {
                for a in &h.attention {
                    if let (Some(t), Some(c)) = (&a.task, &a.card) {
                        out.push(card_item(t.id, c));
                    }
                }
                for (i, a) in h.active.iter().enumerate() {
                    out.push(Item::new(
                        format!("active:{}", a.task),
                        vec![
                            a.model.clone(),
                            a.project.clone(),
                            format!("#{}", a.task),
                            a.step.clone(),
                        ],
                        vec![],
                        i,
                    ));
                }
                out.push(Item::new(
                    "sessions",
                    vec![format!("{} of {}", h.sessions_used, h.sessions_limit)],
                    vec!["increase".into(), "decrease".into()],
                    0,
                ));
            }
            Some(Body::TaskDetail(t)) => {
                if let Some(r) = &t.row {
                    out.push(row_item(r, 0, "detail"));
                }
                for (i, s) in t.steps.iter().enumerate() {
                    out.push(Item::new(
                        format!("step:{}", s.run),
                        vec![s.name.clone(), s.outcome.clone()],
                        vec![],
                        i,
                    ));
                }
                if let (Some(c), Some(r)) = (&t.card, &t.row) {
                    out.push(card_item(r.id, c));
                }
            }
            _ => {}
        }
    }
    out
}

/// Compare what a client shows with what it must show. Returns the problems.
/// A client may show more than the views require (decoration), except rows:
/// an extra row is a task the server did not say is there.
pub fn compare(expected: &[Item], actual: &[Item]) -> Vec<String> {
    let mut problems = vec![];
    let got: BTreeMap<&str, &Item> = actual.iter().map(|i| (i.id.as_str(), i)).collect();
    for e in expected {
        let Some(a) = got.get(e.id.as_str()) else {
            problems.push(format!("missing: {} ({})", e.id, e.texts.join(" | ")));
            continue;
        };
        for t in &e.texts {
            if !a.texts.iter().any(|x| x.contains(t.as_str())) {
                problems.push(format!("{}: does not show {t:?}", e.id));
            }
        }
        if a.actions != e.actions {
            problems.push(format!(
                "{}: offers {:?} but may offer exactly {:?}",
                e.id, a.actions, e.actions
            ));
        }
    }
    let want: BTreeMap<&str, &Item> = expected.iter().map(|i| (i.id.as_str(), i)).collect();
    for a in actual {
        if (a.id.starts_with("row:") || a.id.starts_with("card:"))
            && !want.contains_key(a.id.as_str())
        {
            problems.push(format!(
                "unexpected: {} is shown but the server did not say it is there",
                a.id
            ));
        }
    }
    // Order within each section (sidebar rows) and among tiles is part of the rule.
    let order_of = |items: &[Item], prefix: &str| -> Vec<String> {
        let mut v: Vec<&Item> = items.iter().filter(|i| i.id.starts_with(prefix)).collect();
        v.sort_by_key(|i| i.order);
        v.iter().map(|i| i.id.clone()).collect()
    };
    for prefix in ["section:", "tile:"] {
        let e = order_of(expected, prefix);
        let a: Vec<String> = order_of(actual, prefix)
            .into_iter()
            .filter(|i| e.contains(i))
            .collect();
        let e_present: Vec<String> = e.iter().filter(|i| a.contains(i)).cloned().collect();
        if a != e_present {
            problems.push(format!("{prefix} order is {a:?} but must be {e_present:?}"));
        }
    }
    let sections: Vec<String> = expected
        .iter()
        .filter_map(|i| {
            i.id.strip_prefix("row:")
                .and_then(|r| r.split(':').next())
                .map(String::from)
        })
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    for sec in sections {
        let prefix = format!("row:{sec}:");
        let e = order_of(expected, &prefix);
        let a: Vec<String> = order_of(actual, &prefix)
            .into_iter()
            .filter(|i| e.contains(i))
            .collect();
        if a != e {
            problems.push(format!(
                "rows in {sec} are in the order {a:?}, the server's order is {e:?}"
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, texts: &[&str], actions: &[&str], order: usize) -> Item {
        Item::new(
            id,
            texts.iter().map(|s| s.to_string()).collect(),
            actions.iter().map(|s| s.to_string()).collect(),
            order,
        )
    }

    #[test]
    fn identical_is_clean_and_each_kind_of_difference_is_named() {
        let e = vec![
            item("row:queued:1", &["#1", "A"], &["cancel"], 0),
            item("row:queued:2", &["#2", "B"], &["cancel", "retry"], 1),
        ];
        assert!(compare(&e, &e).is_empty());
        // Missing, wrong text, wrong actions, an extra row, wrong order.
        let a = vec![
            item("row:queued:2", &["#2", "other"], &["cancel"], 0),
            item("row:queued:9", &["#9"], &[], 1),
        ];
        let p = compare(&e, &a);
        let has = |s: &str| p.iter().any(|m| m.contains(s));
        assert!(has("missing: row:queued:1"), "{p:?}");
        assert!(has("does not show \"B\""), "{p:?}");
        assert!(has("offers [\"cancel\"] but may offer exactly"), "{p:?}");
        assert!(has("unexpected: row:queued:9"), "{p:?}");
        let swapped = vec![
            item("row:queued:2", &["#2", "B"], &["cancel", "retry"], 0),
            item("row:queued:1", &["#1", "A"], &["cancel"], 1),
        ];
        assert!(
            compare(&e, &swapped)
                .iter()
                .any(|m| m.contains("rows in queued")),
            "order matters"
        );
    }

    #[test]
    fn extra_decoration_is_fine_but_an_action_too_many_is_not() {
        let e = vec![item("tile:queued", &["4", "queued"], &[], 0)];
        let a = vec![
            item("tile:queued", &["4 queued tasks"], &[], 0),
            item("logo", &["MASON"], &[], 0),
        ];
        assert!(compare(&e, &a).is_empty());
        let a = vec![item("tile:queued", &["4"], &["click"], 0)];
        assert!(!compare(&e, &a).is_empty());
    }

    #[test]
    fn items_round_trip_through_json() {
        let i = item("row:x:1", &["a", "b"], &["z", "a"], 3);
        assert_eq!(Item::from_json(&i.to_json()).unwrap(), i);
        assert_eq!(i.actions, vec!["a", "z"], "actions are kept sorted");
    }
}
