// SPDX-License-Identifier: GPL-3.0-or-later

//! `[watch.ci]`: the base-branch CI watch (decision 0149). Off unless `enabled = true`.

use crate::model::{parse_duration, Problem};
use crate::parse::unknown_key;
use mason_core::ci_watch::Rules;
use toml_edit::Item;

pub const WATCH_KEYS: &[&str] = &["ci"];
pub const WATCH_CI_KEYS: &[&str] = &[
    "enabled",
    "workflows",
    "exclude",
    "gate",
    "agreement",
    "red_ticket_after",
    "max_open_auto",
    "freeze_on_gate",
];

fn strings(item: &Item, path: &str, out: &mut Vec<Problem>) -> Vec<String> {
    let Some(a) = item.as_array() else {
        out.push(Problem::new(path, "must be a list of strings"));
        return vec![];
    };
    let mut v = vec![];
    for x in a.iter() {
        match x.as_str() {
            Some(s) if !s.trim().is_empty() => v.push(s.to_string()),
            _ => out.push(Problem::new(path, "must be a list of non-empty strings")),
        }
    }
    v
}

/// The rules when the watch is switched on, `None` when it is off or absent.
pub fn parse(item: &Item, out: &mut Vec<Problem>) -> Option<Rules> {
    let Some(t) = item.as_table_like() else {
        out.push(Problem::new("watch", "must be a table, like [watch.ci]"));
        return None;
    };
    let mut ci = None;
    for (k, v) in t.iter() {
        match k {
            "ci" => ci = Some(v),
            other => out.push(unknown_key("watch", other, WATCH_KEYS)),
        }
    }
    let ci = ci?;
    let Some(c) = ci.as_table_like() else {
        out.push(Problem::new("watch.ci", "must be a table"));
        return None;
    };
    let mut r = Rules {
        workflows: vec![],
        exclude: vec![],
        gate: None,
        agreement: 2,
        red_ticket_after_ms: 3 * 3600 * 1000,
        max_open_auto: 2,
        freeze_on_gate: true,
    };
    let mut enabled = false;
    let int = |v: &Item, path: &str, out: &mut Vec<Problem>, min: i64| -> Option<u32> {
        match v.as_integer() {
            Some(n) if n >= min && n <= i64::from(u32::MAX) => Some(n as u32),
            _ => {
                out.push(Problem::new(
                    path,
                    format!("must be a whole number, {min} or more"),
                ));
                None
            }
        }
    };
    for (k, v) in c.iter() {
        let path = format!("watch.ci.{k}");
        match k {
            "enabled" => match v.as_bool() {
                Some(b) => enabled = b,
                None => out.push(Problem::new(&path, "must be true or false")),
            },
            "workflows" => r.workflows = strings(v, &path, out),
            "exclude" => r.exclude = strings(v, &path, out),
            "gate" => match v.as_str() {
                Some(s) if !s.trim().is_empty() => r.gate = Some(s.trim().to_string()),
                _ => out.push(Problem::new(
                    &path,
                    "must be a workflow name, or `workflow/job`",
                )),
            },
            "agreement" => {
                if let Some(n) = int(v, &path, out, 1) {
                    r.agreement = n;
                }
            }
            "red_ticket_after" => match v.as_str().and_then(parse_duration) {
                Some(d) => r.red_ticket_after_ms = d.as_millis() as u64,
                None => out.push(Problem::new(&path, "must be a duration, like \"3h\"")),
            },
            "max_open_auto" => {
                if let Some(n) = int(v, &path, out, 0) {
                    r.max_open_auto = n;
                }
            }
            "freeze_on_gate" => match v.as_bool() {
                Some(b) => r.freeze_on_gate = b,
                None => out.push(Problem::new(&path, "must be true or false")),
            },
            other => out.push(unknown_key("watch.ci", other, WATCH_CI_KEYS)),
        }
    }
    enabled.then_some(r)
}
