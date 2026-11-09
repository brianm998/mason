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

fn hash_list(ids: &[u64]) -> String {
    ids.iter()
        .map(|n| format!("#{n}"))
        .collect::<Vec<_>>()
        .join(", ")
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
    // The tier and the real model, as the daemon wrote it.
    if !r.display.is_empty() {
        texts.push(r.display.clone());
    }
    // A task the tracker imported shows its issue, and where to read it.
    if r.issue != 0 {
        texts.push(format!("issue #{}", r.issue));
        if !r.issue_url.is_empty() {
            texts.push(r.issue_url.clone());
        }
    }
    // The task page says what it waits for and what waits for it.
    if prefix == "detail" {
        if !r.blockers.is_empty() {
            texts.push(format!("waiting for {}", hash_list(&r.blockers)));
        }
        if !r.blocks.is_empty() {
            texts.push(format!("blocks {}", hash_list(&r.blocks)));
        }
    }
    Item::new(
        format!("{prefix}:{}", r.id),
        texts,
        r.allowed_actions.clone(),
        order,
    )
}

/// What a held task's page shows for its blockers: each one with its state and where it came
/// from, and the actions on it (the owner's alone); and the Add blocker field.
pub fn blocker_items(r: &TaskRow, owner: bool) -> Vec<Item> {
    let mut out = vec![];
    for (i, l) in r.blocker_links.iter().enumerate() {
        out.push(Item::new(
            format!("blocker:{}:{}", r.id, l.task),
            vec![format!("#{}", l.task), l.state.clone(), l.origin.clone()],
            if owner { l.actions.clone() } else { vec![] },
            i,
        ));
    }
    if r.allowed_actions.iter().any(|a| a == "block") {
        out.push(Item::new(
            format!("add-blocker:{}", r.id),
            vec!["Add blocker".into()],
            if owner {
                vec!["add_blocker".into()]
            } else {
                vec![]
            },
            0,
        ));
    }
    out
}

fn src(s: i32) -> mason_core::progress::Source {
    use mason_core::progress::Source;
    match ProgressSource::try_from(s).unwrap_or(ProgressSource::SourceNone) {
        ProgressSource::SourceNone => Source::None,
        ProgressSource::SourceReported => Source::Reported,
        ProgressSource::SourceCounted => Source::Counted,
        ProgressSource::SourceHistory => Source::History,
        ProgressSource::SourceExpected => Source::Expected,
        ProgressSource::SourceMinimum => Source::Minimum,
    }
}

/// The count of a step ("212 of 340 tests, 62%") by the rule for numbers.
pub fn cell_count_text(c: &ProgressCell) -> String {
    mason_core::progress_text::count_text(
        &c.unit,
        c.done as u64,
        src(c.done_source),
        c.total as u64,
        src(c.total_source),
        c.total_low as u64,
    )
}

/// What a running step's progress says, without any clock text: the count, the message, where
/// each number came from, the output figures, each sub-count, the todo list, and "silent" when
/// the step is quiet past its usual gap (judged at `now_ms`).
pub fn progress_texts(c: &ProgressCell, now_ms: u64) -> Vec<String> {
    use mason_core::progress::is_stale;
    use mason_core::progress_text::{count_text, group, source_words};
    let mut t = vec![];
    let count = cell_count_text(c);
    if !count.is_empty() {
        t.push(count);
    }
    if !c.message.is_empty() {
        t.push(c.message.clone());
    }
    if c.done_source != 0 {
        t.push(format!(
            "count {}",
            source_words(src(c.done_source), c.history_samples)
        ));
    }
    if c.total_source != 0 {
        t.push(format!(
            "total {}",
            source_words(src(c.total_source), c.history_samples)
        ));
    }
    if c.lines > 0 || c.bytes > 0 {
        t.push(format!(
            "output {} lines, {} bytes",
            group(c.lines),
            group(c.bytes)
        ));
    }
    if let Some(k) = &c.tokens {
        let o = |has: bool, v: u64| has.then_some(v);
        let (i, out, cr, cw) = (
            o(k.has_input, k.input),
            o(k.has_output, k.output),
            o(k.has_cache_read, k.cache_read),
            o(k.has_cache_write, k.cache_write),
        );
        let compact = mason_core::progress_text::tokens_compact(i, out, cr, cw);
        if !compact.is_empty() {
            t.push(format!("tokens {compact}"));
        }
        t.extend(mason_core::progress_text::tokens_detail(
            i,
            out,
            cr,
            cw,
            k.has_cost.then_some(k.cost_usd),
        ));
    }
    for s in &c.subs {
        t.push(format!(
            "{}: {}",
            s.name,
            count_text(
                &s.unit,
                s.done as u64,
                src(s.done_source),
                s.total as u64,
                src(s.total_source),
                0
            )
        ));
    }
    for i in &c.todo {
        t.push(format!("{}: {}", i.status, i.text));
    }
    if is_stale(now_ms, c.updated_at_ms, c.stale_after_ms) {
        t.push("silent".into());
    }
    t
}

fn progress_items(r: &TaskRow, now_ms: u64) -> Vec<Item> {
    r.progress
        .iter()
        .filter(|c| c.state == CellState::Part as i32 && c.run != 0)
        .map(|c| {
            Item::new(
                format!("progress:{}:{}", r.id, c.step),
                progress_texts(c, now_ms),
                vec![],
                0,
            )
        })
        .collect()
}

fn system_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn card_item(task: u64, c: &DecisionCard, owner: bool) -> Item {
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
    // Pick an option, or type an answer: what the card offers the owner. Anyone
    // else sees the card and no buttons.
    let mut actions: Vec<String> = vec![];
    if owner {
        actions = (1..=c.options.len())
            .map(|i| format!("option:{i}"))
            .collect();
        actions.push("other".into());
    }
    Item::new(format!("card:{task}"), texts, actions, 0)
}

/// What a task waiting for the owner's approval shows: what it waits on, in full, and
/// Approve and Reject for the owner only.
fn approval_item(task: u64, a: &Approval, owner: bool) -> Item {
    let mut texts = vec![a.waiting_for.clone()];
    texts.extend(a.lines.iter().cloned());
    if !a.diff_hash.is_empty() {
        texts.push(format!("diff {}", a.diff_hash));
    }
    if !a.branch.is_empty() {
        texts.push(a.branch.clone());
    }
    if !a.diff_command.is_empty() {
        texts.push(a.diff_command.clone());
    }
    let actions = if owner {
        vec!["approve".to_string(), "reject".to_string()]
    } else {
        vec![]
    };
    Item::new(format!("approval:{task}"), texts, actions, 0)
}

/// Everything the views say is visible, as a flat list. `owner` says whether the
/// client's connection acts as the owner (the Welcome says so): only then are the
/// answer, approve and reject buttons offered.
pub fn visible(views: &[View], owner: bool) -> Vec<Item> {
    let mut out = vec![];
    for v in views {
        match v.body.as_ref() {
            Some(Body::Header(h)) => {
                out.push(Item::new("status", vec![h.status.clone()], vec![], 0));
                for (i, c) in h.counts.iter().enumerate() {
                    out.push(Item::new(
                        format!("tile:{}", c.state),
                        vec![
                            c.count.to_string(),
                            if c.label.is_empty() {
                                c.state.replace('_', " ")
                            } else {
                                c.label.clone()
                            },
                        ],
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
                // Issues the tracker has not imported: a count tile and a list, only while true.
                if !h.pending_issues.is_empty() {
                    out.push(Item::new(
                        "tile:not_imported",
                        vec![h.pending_issues.len().to_string(), "not imported".into()],
                        vec![],
                        h.counts.len(),
                    ));
                    for (i, p) in h.pending_issues.iter().enumerate() {
                        let mut texts = vec![
                            p.project.clone(),
                            format!("#{}", p.number),
                            p.title.clone(),
                            p.reason.clone(),
                        ];
                        if !p.url.is_empty() {
                            texts.push(p.url.clone());
                        }
                        out.push(Item::new(
                            format!("pending:{}:{}", p.project, p.number),
                            texts,
                            vec![],
                            i,
                        ));
                    }
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
                            out.extend(progress_items(r, system_now_ms()));
                        }
                    }
                }
            }
            Some(Body::Home(h)) => {
                for a in &h.attention {
                    if let (Some(t), Some(c)) = (&a.task, &a.card) {
                        out.push(card_item(t.id, c, owner));
                    }
                    if let (Some(t), Some(x)) = (&a.task, &a.approval) {
                        out.push(approval_item(t.id, x, owner));
                    }
                }
                for (i, a) in h.active.iter().enumerate() {
                    out.push(Item::new(
                        format!("active:{}", a.task),
                        vec![
                            if a.display.is_empty() {
                                a.model.clone()
                            } else {
                                a.display.clone()
                            },
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
                    out.extend(progress_items(r, system_now_ms()));
                    out.extend(blocker_items(r, owner));
                }
                for (i, s) in t.steps.iter().enumerate() {
                    let mut texts = vec![s.name.clone(), s.outcome.clone()];
                    texts.extend(s.launches.iter().map(|l| l.display.clone()));
                    out.push(Item::new(format!("step:{}", s.run), texts, vec![], i));
                }
                if let (Some(c), Some(r)) = (&t.card, &t.row) {
                    out.push(card_item(r.id, c, owner));
                }
                if let (Some(a), Some(r)) = (&t.approval, &t.row) {
                    out.push(approval_item(r.id, a, owner));
                }
            }
            Some(Body::Editor(e)) => out.extend(editor_items(e)),
            Some(Body::Usage(u)) => out.extend(usage_items(u)),
            _ => {}
        }
    }
    out
}

/// The percentage text of a window, as both monitors word it.
pub fn window_percent_text(w: &UsageWindow) -> String {
    if !w.reported {
        "not reported yet".into()
    } else if w.has_percent {
        format!("{}%", w.percent.round() as i64)
    } else {
        "no percentage reported".into()
    }
}

/// What the LLM usage page shows (s64), as the texts that do not move with the
/// clock: each account with its adapter, billing and sessions, and each window
/// with its label, percentage (or that there is none) and state. The ages and
/// countdowns change every minute and are not compared.
pub fn usage_items(u: &UsageView) -> Vec<Item> {
    let mut out = vec![];
    for (i, p) in u.providers.iter().enumerate() {
        out.push(Item::new(
            format!("usage:{}", p.name),
            vec![
                p.name.clone(),
                p.adapter.clone(),
                p.billing.clone(),
                format!("{} of {} sessions", p.sessions_used, p.sessions_limit),
            ],
            vec![],
            i,
        ));
        for (j, w) in p.windows.iter().enumerate() {
            let mut texts = vec![w.label.clone(), window_percent_text(w)];
            if w.reported {
                texts.push(w.state.clone());
            }
            out.push(Item::new(
                format!("usage-window:{}:{}", p.name, w.id),
                texts,
                vec![],
                j,
            ));
        }
    }
    out
}

/// What the pipeline editor shows (Part 18): the lanes, each node with its problems,
/// each edge, each row of the test grid with the places its level runs, the problems that
/// belong to no node, and the toolbar: what may be done to the draft now.
pub fn editor_items(e: &mason_protocol::EditorView) -> Vec<Item> {
    let mut out = vec![];
    for (i, l) in e.lanes.iter().enumerate() {
        out.push(Item::new(
            format!("lane:{}", l.category),
            vec![l.title.clone()],
            vec![],
            i,
        ));
    }
    for (i, n) in e.nodes.iter().enumerate() {
        let mut texts = vec![n.name.clone(), n.kind.clone(), n.category.clone()];
        if !n.summary.is_empty() {
            texts.push(n.summary.clone());
        }
        if n.is_start {
            texts.push("start".into());
        }
        texts.extend(n.problems.iter().cloned());
        out.push(Item::new(format!("node:{}", n.name), texts, vec![], i));
    }
    for (i, x) in e.edges.iter().enumerate() {
        let mut texts = vec![
            x.from.clone(),
            x.to.clone(),
            if x.kind == "on_red" {
                "on red".to_string()
            } else {
                "next".to_string()
            },
        ];
        if x.bound > 0 {
            texts.push(format!("at most {} times", x.bound));
        }
        out.push(Item::new(
            format!("edge:{}:{}:{}", x.from, x.kind, x.to),
            texts,
            vec![],
            i,
        ));
    }
    if let Some(g) = &e.grid {
        for (i, r) in g.rows.iter().enumerate() {
            let mut texts = vec![r.suite.clone(), r.level.clone()];
            texts.extend(
                g.columns
                    .iter()
                    .zip(&r.cells)
                    .filter(|(_, on)| **on)
                    .map(|(c, _)| c.replace('_', " ")),
            );
            out.push(Item::new(format!("grid:{}", r.suite), texts, vec![], i));
        }
    }
    for (i, p) in e.problems.iter().enumerate() {
        out.push(Item::new(
            format!("problem:{i}"),
            vec![p.clone()],
            vec![],
            i,
        ));
    }
    let mut actions = vec!["simulate".to_string()];
    if e.can_undo {
        actions.push("undo".into());
    }
    if e.can_redo {
        actions.push("redo".into());
    }
    if e.dirty {
        actions.push("discard".into());
        actions.push("apply".into());
    }
    out.push(Item::new(
        "draft",
        vec![if e.dirty {
            "changed".to_string()
        } else {
            "unchanged".to_string()
        }],
        actions,
        0,
    ));
    for (i, c) in e.changes.iter().enumerate() {
        out.push(Item::new(format!("change:{i}"), vec![c.clone()], vec![], i));
    }
    out
}

/// A banner's text with the clock taken out: the time of day (`16:35`), the
/// weekday before it, and a countdown (`(in 2 h 10 m)`). The expected views and
/// a client's capture come from two runs of the demo, minutes apart, and a
/// banner such as an account hold names a reset relative to when its run
/// started. What the banner says, and which banner it is, still has to match.
pub fn steady(text: &str) -> String {
    const DAYS: [&str; 7] = ["Mon ", "Tue ", "Wed ", "Thu ", "Fri ", "Sat ", "Sun "];
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < b.len() {
        let rest = &text[i..];
        if rest.starts_with("(in ") {
            if let Some(end) = rest.find(')') {
                out.push_str("(in …)");
                i += end + 1;
                continue;
            }
        }
        // A weekday right before a time of day.
        if let Some(d) = DAYS.iter().find(|d| rest.starts_with(**d)) {
            if clock_len(&text[i + d.len()..]).is_some() {
                out.push_str("DAY ");
                i += d.len();
                continue;
            }
        }
        let after_digit = i > 0 && b[i - 1].is_ascii_digit();
        if !after_digit {
            if let Some(n) = clock_len(rest) {
                out.push_str("HH:MM");
                i += n;
                continue;
            }
        }
        let c = rest.chars().next().expect("not at the end");
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// The length of a time of day (`9:05`, `16:35`) at the start of `s`.
fn clock_len(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    let h = b.iter().take_while(|c| c.is_ascii_digit()).count();
    if !(1..=2).contains(&h) || b.get(h) != Some(&b':') {
        return None;
    }
    let m = b[h + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
    (m == 2).then_some(h + 3)
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
        let banner = e.id.starts_with("banner:");
        for t in &e.texts {
            let shown = if banner {
                let want = steady(t);
                a.texts.iter().any(|x| steady(x).contains(want.as_str()))
            } else {
                a.texts.iter().any(|x| x.contains(t.as_str()))
            };
            if !shown {
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
        if (a.id.starts_with("row:") || a.id.starts_with("card:") || a.id.starts_with("approval:"))
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
    fn a_banner_is_compared_without_its_clock() {
        let e = vec![item(
            "banner:0",
            &["claude-full: 5-hour limit, resets Sun 16:35 UTC (in 2 h 10 m); waiting tasks start no sessions until then"],
            &[],
            0,
        )];
        let later = vec![item(
            "banner:0",
            &["claude-full: 5-hour limit, resets Sun 16:37 UTC (in 2 h 9 m); waiting tasks start no sessions until then"],
            &[],
            0,
        )];
        assert!(compare(&e, &later).is_empty(), "{:?}", compare(&e, &later));
        // What the banner says still has to match.
        let other = vec![item(
            "banner:0",
            &["claude-main: 5-hour limit, resets Sun 16:37 UTC (in 2 h 9 m); waiting tasks start no sessions until then"],
            &[],
            0,
        )];
        assert_eq!(compare(&e, &other).len(), 1);
        // Only banners: any other item still compares its text exactly.
        let e = vec![item("tile:queued", &["resets 16:35"], &[], 0)];
        let a = vec![item("tile:queued", &["resets 16:37"], &[], 0)];
        assert_eq!(compare(&e, &a).len(), 1);
        assert_eq!(
            steady("at 9:05, 10:30x and 123:45"),
            "at HH:MM, HH:MMx and 123:45"
        );
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
    fn only_the_owner_is_offered_the_buttons() {
        let card = DecisionCard {
            ask: "Which?".into(),
            why: "because".into(),
            options: vec![
                CardOption {
                    label: "A".into(),
                    recommended: true,
                },
                CardOption {
                    label: "B".into(),
                    recommended: false,
                },
            ],
            evidence: vec![],
            hash: "h".into(),
            origin: "agent".into(),
        };
        let owner = card_item(7, &card, true);
        assert_eq!(owner.actions, vec!["option:1", "option:2", "other"]);
        assert!(owner.texts.contains(&"1. A (recommended)".to_string()));
        let other = card_item(7, &card, false);
        assert!(other.actions.is_empty(), "a non-owner sees the card only");
        assert_eq!(other.texts, owner.texts);
        let a = Approval {
            kind: "hold".into(),
            waiting_for: "Your approval".into(),
            lines: vec!["touches crates/x.rs (a gate)".into()],
            diff_hash: "abc".into(),
            branch: "mason/p/t1".into(),
            diff_command: "git diff main...mason/p/t1".into(),
        };
        assert_eq!(
            approval_item(7, &a, true).actions,
            vec!["approve", "reject"]
        );
        assert!(approval_item(7, &a, false).actions.is_empty());
        assert!(approval_item(7, &a, true)
            .texts
            .contains(&"touches crates/x.rs (a gate)".to_string()));
    }

    #[test]
    fn items_round_trip_through_json() {
        let i = item("row:x:1", &["a", "b"], &["z", "a"], 3);
        assert_eq!(Item::from_json(&i.to_json()).unwrap(), i);
        assert_eq!(i.actions, vec!["a", "z"], "actions are kept sorted");
    }
}

#[cfg(test)]
mod progress_tests {
    use super::*;

    fn cell() -> ProgressCell {
        ProgressCell {
            step: "develop".into(),
            state: CellState::Part as i32,
            unit: "tests".into(),
            done: 212,
            total: 340,
            done_source: ProgressSource::SourceReported as i32,
            total_source: ProgressSource::SourceReported as i32,
            message: "running the api suite".into(),
            lines: 1240,
            bytes: 61000,
            updated_at_ms: 1_000_000,
            run: 3,
            stale_after_ms: 300_000,
            ..Default::default()
        }
    }

    #[test]
    fn a_real_count_has_a_percentage_and_sources() {
        let t = progress_texts(&cell(), 1_004_000);
        assert_eq!(t[0], "212 of 340 tests, 62%");
        assert!(t.contains(&"count from the agent".to_string()));
        assert!(t.contains(&"output 1,240 lines, 61,000 bytes".to_string()));
        assert!(!t.contains(&"silent".to_string()));
        assert!(t.iter().all(|x| !x.contains(" ago")));
    }

    #[test]
    fn a_typical_total_has_no_percentage() {
        let mut c = cell();
        c.done_source = ProgressSource::SourceCounted as i32;
        c.total_source = ProgressSource::SourceHistory as i32;
        c.unit = "lines".into();
        c.done = 1240;
        c.total = 2400;
        c.total_low = 1800;
        c.history_samples = 12;
        let t = progress_texts(&c, 1_004_000);
        assert_eq!(t[0], "1,240 lines, usually 1,800 to 2,400");
        assert!(t.contains(&"total usually, from 12 past runs".to_string()));
        assert!(t.iter().all(|x| !x.contains('%')));
    }

    #[test]
    fn a_quiet_step_is_silent_and_a_planted_change_differs() {
        let c = cell();
        assert!(progress_texts(&c, 1_000_000 + 300_001).contains(&"silent".to_string()));
        assert!(!progress_texts(&c, 1_000_000 + 300_000).contains(&"silent".to_string()));
        let a = vec![Item::new(
            "progress:1:develop",
            progress_texts(&c, 1_004_000),
            vec![],
            0,
        )];
        let mut d = c.clone();
        d.done = 213;
        let b = vec![Item::new(
            "progress:1:develop",
            progress_texts(&d, 1_004_000),
            vec![],
            0,
        )];
        assert!(!compare(&a, &b).is_empty());
    }
}
