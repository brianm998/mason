// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason disk` and `mason disk clean`: what the pipeline's worktrees take and what the
//! daemon's sweep removes (s1145).

use serde_json::Value;

/// Polls of a report still being measured before it is printed as it is (about ten minutes).
const MAX_POLLS: u32 = 300;

/// Ask for the report until it is measured. `wait` is called between polls.
pub fn report<E>(
    mut get: impl FnMut() -> Result<Value, E>,
    mut wait: impl FnMut(),
) -> Result<Value, E> {
    let mut r = get()?;
    for n in 0..MAX_POLLS {
        if r["measuring"] != true {
            break;
        }
        if n == 0 {
            eprintln!("measuring the worktrees…");
        }
        wait();
        r = get()?;
    }
    Ok(r)
}

/// "412 GB", "1.5 MB": decimal units, as the monitor shows them.
pub fn bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut u = 0;
    while u + 1 < units.len() && v >= 999.5 {
        v /= 1000.0;
        u += 1;
    }
    if u > 0 && v < 9.95 {
        format!("{v:.1} {}", units[u])
    } else {
        format!("{v:.0} {}", units[u])
    }
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or_default()
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    v[k].as_array().map_or(&[], Vec::as_slice)
}

/// "mason #12 (canceled)", or a landing worktree by its directory.
fn name(i: &Value) -> String {
    let (project, state) = (str_of(i, "project"), str_of(i, "state"));
    match i["task"].as_u64() {
        Some(t) => format!("{project} #{t} ({state})"),
        None => {
            let dir = str_of(i, "path").rsplit('/').next().unwrap_or_default();
            format!("{project} {dir} ({state})")
        }
    }
}

/// One line for an item with an action. `past`: it has been done.
fn action_line(i: &Value, past: bool) -> Option<String> {
    let action = i["action"].as_str()?;
    let size = i["frees_bytes"]
        .as_u64()
        .map_or(String::new(), |b| format!("  {}", bytes(b)));
    let what = match action {
        "trim" => {
            let dirs: Vec<&str> = list(i, "outputs")
                .iter()
                .filter_map(Value::as_str)
                .collect();
            format!("  {}", dirs.join(", "))
        }
        _ => String::new(),
    };
    let verb = match (action, past) {
        ("trim", false) => "trim",
        ("trim", true) => "trimmed",
        (_, false) => "remove",
        (_, true) => "removed",
    };
    Some(format!(
        "  {verb:<8}{}{size}{what}  {}\n",
        name(i),
        str_of(i, "why")
    ))
}

fn skipped_lines(items: &[Value]) -> String {
    let lines: String = items
        .iter()
        .filter_map(|i| {
            let why = i["skipped"].as_str()?;
            Some(format!("  {}: {why}  {}\n", name(i), str_of(i, "path")))
        })
        .collect();
    if lines.is_empty() {
        lines
    } else {
        format!("Left alone:\n{lines}")
    }
}

/// The text of `mason disk`.
pub fn report_text(r: &Value) -> String {
    let items = list(r, "items");
    let mut out = format!(
        "MASON worktrees: {} in {} directories; the next sweep would free {}{}\n",
        bytes(r["bytes"].as_u64().unwrap_or(0)),
        items.len(),
        bytes(r["free_next_bytes"].as_u64().unwrap_or(0)),
        if r["measuring"] == true {
            " (still measuring)"
        } else {
            ""
        },
    );
    if !list(r, "totals").is_empty() {
        out.push_str(&format!(
            "{:<18}{:<17}{:>5}{:>10}{:>18}\n",
            "kind", "state", "dirs", "size", "next sweep frees"
        ));
    }
    for t in list(r, "totals") {
        out.push_str(&format!(
            "{:<18}{:<17}{:>5}{:>10}{:>18}\n",
            str_of(t, "kind"),
            str_of(t, "state"),
            t["count"].as_u64().unwrap_or(0),
            bytes(t["bytes"].as_u64().unwrap_or(0)),
            bytes(t["free_next_bytes"].as_u64().unwrap_or(0)),
        ));
    }
    let next: String = items.iter().filter_map(|i| action_line(i, false)).collect();
    if !next.is_empty() {
        out.push_str("Next sweep:\n");
        out.push_str(&next);
    }
    out.push_str(&skipped_lines(items));
    out
}

/// The text of `mason disk clean`.
pub fn clean_text(r: &Value) -> String {
    let dry = r["dry_run"] == true;
    let items = list(r, "items");
    let done: String = items.iter().filter_map(|i| action_line(i, !dry)).collect();
    let mut out = match (done.is_empty(), dry) {
        (true, _) => "nothing to remove\n".to_string(),
        (false, true) => format!("Would remove (nothing was removed):\n{done}"),
        (false, false) => format!("Removed (the space comes back as it is deleted):\n{done}"),
    };
    out.push_str(&skipped_lines(items));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn items() -> Value {
        json!([
            {"project": "mason", "task": 12, "state": "canceled", "path": "/d/w/mason/12",
             "action": "remove", "why": "canceled 30 h ago (kept 24 h)", "frees_bytes": 4_100_000_000u64},
            {"project": "mason", "task": 14, "state": "developing", "path": "/d/w/mason/14",
             "action": "trim", "why": "idle 13 h", "outputs": ["target"], "frees_bytes": 1_048_576},
            {"project": "mason", "task": 20, "state": "canceled", "path": "/d/w/mason/20",
             "action": null, "skipped": "uncommitted changes (2 paths)"},
        ])
    }

    #[test]
    fn the_report_lists_totals_what_goes_next_and_what_is_left() {
        let r = json!({"items": items(), "bytes": 5_000_000_000u64, "free_next_bytes": 4_101_048_576u64,
            "totals": [{"kind": "task worktree", "state": "canceled", "count": 2,
                        "bytes": 5_000_000_000u64, "free_next_bytes": 4_100_000_000u64}]});
        let t = report_text(&r);
        for want in [
            "5.0 GB in 3 directories; the next sweep would free 4.1 GB",
            "task worktree     canceled",
            "remove  mason #12 (canceled)  4.1 GB",
            "trim    mason #14 (developing)  1.0 MB  target  idle 13 h",
            "mason #20 (canceled): uncommitted changes (2 paths)",
        ] {
            assert!(t.contains(want), "{want:?} not in:\n{t}");
        }
    }

    #[test]
    fn a_dry_run_says_it_removed_nothing() {
        let t = clean_text(&json!({"dry_run": true, "items": items()}));
        assert!(t.starts_with("Would remove (nothing was removed):"), "{t}");
        assert!(t.contains("remove  mason #12"), "{t}");
        let t = clean_text(&json!({"dry_run": false, "items": items()}));
        assert!(t.contains("removed mason #12"), "{t}");
        assert!(clean_text(&json!({"items": []})).starts_with("nothing to remove"));
    }

    #[test]
    fn a_report_is_asked_for_until_it_is_measured() {
        let mut n = 0;
        let r: Result<Value, ()> = report(
            || {
                n += 1;
                Ok(json!({"measuring": n < 3}))
            },
            || {},
        );
        assert_eq!((r.unwrap()["measuring"].clone(), n), (json!(false), 3));
    }

    #[test]
    fn sizes_are_decimal() {
        for (n, want) in [
            (0, "0 B"),
            (999, "999 B"),
            (1_500, "1.5 KB"),
            (260_000_000_000, "260 GB"),
        ] {
            assert_eq!(bytes(n), want);
        }
    }
}
