// SPDX-License-Identifier: GPL-3.0-or-later

//! `clients/features.toml`: every feature and its status in each client. A
//! feature marked done in only one client fails the check unless it has a
//! recorded platform reason; "n/a" always needs one.

use toml_edit::{DocumentMut, Item};

pub const CLIENTS: [&str; 3] = ["swift", "compose", "ios"];

/// The tables that hold deliberate differences, by the surface they apply to.
const IPAD_TABLE: &str = "ipad";
const PHONE_TABLE: &str = "phone";

#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub features: usize,
    pub done: [usize; 3],
    pub problems: Vec<String>,
}

/// Checks that every `[<table>.*]` entry records a platform_reason; returns the names.
fn differences(doc: &DocumentMut, table: &str, problems: &mut Vec<String>) -> Vec<String> {
    let mut names = vec![];
    let Some(t) = doc.get(table).and_then(Item::as_table_like) else {
        return names;
    };
    for (name, item) in t.iter() {
        names.push(name.to_string());
        let get = |k: &str| {
            item.as_table_like()
                .and_then(|t| t.get(k))
                .and_then(Item::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        if get("platform_reason").is_empty() {
            problems.push(format!(
                "{table}.{name} is a deliberate difference with no platform_reason"
            ));
        }
        if get("description").is_empty() {
            problems.push(format!("{table}.{name} has no description"));
        }
    }
    names
}

pub fn check(text: &str) -> Result<Report, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
    let feats = doc
        .get("feature")
        .and_then(|f| f.as_table_like())
        .ok_or("no [feature.*] tables")?;
    let mut r = Report {
        features: 0,
        done: [0; 3],
        problems: vec![],
    };
    let ipad = differences(&doc, IPAD_TABLE, &mut r.problems);
    let phone = differences(&doc, PHONE_TABLE, &mut r.problems);
    for (name, item) in feats.iter() {
        let t = item
            .as_table_like()
            .ok_or_else(|| format!("feature.{name} must be a table"))?;
        r.features += 1;
        let get = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let reason = get("platform_reason").trim();
        let mut status = [""; 3];
        for (i, c) in CLIENTS.iter().enumerate() {
            let s = get(c);
            let ok = matches!(s, "done" | "todo" | "n/a") || (*c == "ios" && s == "difference");
            if !ok {
                r.problems.push(format!(
                    "feature.{name}.{c} is {s:?}; it must be \"done\", \"todo\" or \"n/a\"{}",
                    if *c == "ios" {
                        " (ios may also be \"difference\")"
                    } else {
                        ""
                    }
                ));
            }
            if s == "done" {
                r.done[i] += 1;
            }
            status[i] = s;
        }
        if get("description").trim().is_empty() {
            r.problems
                .push(format!("feature.{name} has no description"));
        }
        if status[2] == "difference" {
            for (key, table, known) in [
                ("ios_ipad", IPAD_TABLE, &ipad),
                ("ios_iphone", PHONE_TABLE, &phone),
            ] {
                let v = get(key);
                let named = v.strip_prefix(&format!("{table}.")).unwrap_or("");
                if v != "done" && !known.iter().any(|k| k == named) {
                    r.problems.push(format!(
                        "feature.{name}.{key} is {v:?}; it must be \"done\" or name a [{table}.*] deliberate difference"
                    ));
                }
            }
        }
        // The two desktop clients are compared with each other; ios is compared with swift
        // (the views it shares). A valid "difference" is recorded data, not an imbalance.
        let uneven = status[0] != status[1];
        let ios_uneven = status[2] != "difference" && status[2] != status[0];
        let na = status.contains(&"n/a");
        if (uneven || ios_uneven) && reason.is_empty() {
            r.problems.push(format!(
                "feature.{name}: swift is {:?}, compose is {:?} and ios is {:?}, with no platform_reason. Finish it in every client, or record why one cannot have it",
                status[0], status[1], status[2]
            ));
        }
        if na && reason.is_empty() {
            r.problems.push(format!(
                "feature.{name} is n/a for a client with no platform_reason"
            ));
        }
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(sw: &str, co: &str, reason: &str) -> String {
        format!(
            "[feature.x]\ndescription = \"d\"\nswift = \"{sw}\"\ncompose = \"{co}\"\nios = \"{sw}\"\n{}",
            if reason.is_empty() {
                String::new()
            } else {
                format!("platform_reason = \"{reason}\"\n")
            }
        )
    }

    #[test]
    fn balanced_features_pass() {
        for (a, b) in [("done", "done"), ("todo", "todo")] {
            assert!(
                check(&feat(a, b, "")).unwrap().problems.is_empty(),
                "{a} {b}"
            );
        }
    }

    #[test]
    fn done_in_one_client_only_fails_without_a_reason() {
        let r = check(&feat("done", "todo", "")).unwrap();
        assert_eq!(r.problems.len(), 1);
        assert!(
            r.problems[0].contains("no platform_reason"),
            "{:?}",
            r.problems
        );
        assert_eq!(r.done, [1, 0, 1]);
        assert!(check(&feat("todo", "done", "")).unwrap().problems.len() == 1);
    }

    #[test]
    fn a_recorded_platform_reason_excuses_an_imbalance_and_na_needs_one() {
        assert!(check(&feat("done", "n/a", "Android has no menu bar"))
            .unwrap()
            .problems
            .is_empty());
        assert!(check(&feat("done", "todo", "Compose cannot do this yet"))
            .unwrap()
            .problems
            .is_empty());
        assert!(!check(&feat("n/a", "n/a", "")).unwrap().problems.is_empty());
    }

    #[test]
    fn mistakes_in_the_file_are_named() {
        assert!(check("[feature.x]\nswift = \"done\"\ncompose = \"done\"\n")
            .unwrap()
            .problems
            .iter()
            .any(|p| p.contains("no description")));
        assert!(check(&feat("finished", "done", ""))
            .unwrap()
            .problems
            .iter()
            .any(|p| p.contains("must be")));
        assert!(check("x = 1").is_err());
        assert!(check("not toml [").is_err());
    }

    const TABLES: &str = "[ipad.g]\ndescription = \"d\"\nplatform_reason = \"read-only\"\n[phone.g]\ndescription = \"d\"\nplatform_reason = \"small\"\n";

    fn ios(extra: &str) -> String {
        format!(
            "[feature.x]\ndescription = \"d\"\nswift = \"done\"\ncompose = \"done\"\n{extra}\n{TABLES}"
        )
    }

    #[test]
    fn a_feature_missing_on_ios_with_no_difference_fails() {
        for extra in ["ios = \"todo\"", "ios = \"n/a\"", ""] {
            let r = check(&ios(extra)).unwrap();
            assert!(!r.problems.is_empty(), "{extra}: {:?}", r.problems);
        }
        assert!(check(&ios("ios = \"done\"")).unwrap().problems.is_empty());
    }

    #[test]
    fn a_phone_or_ipad_difference_passes() {
        let r = check(&ios(
            "ios = \"difference\"\nios_ipad = \"ipad.g\"\nios_iphone = \"phone.g\"",
        ))
        .unwrap();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        let r = check(&ios(
            "ios = \"difference\"\nios_ipad = \"done\"\nios_iphone = \"phone.g\"",
        ))
        .unwrap();
        assert!(r.problems.is_empty(), "{:?}", r.problems);
        assert_eq!(r.done, [1, 1, 0]);
    }

    #[test]
    fn a_difference_must_name_a_known_table_for_its_surface() {
        for extra in [
            "ios = \"difference\"\nios_ipad = \"ipad.nope\"\nios_iphone = \"phone.g\"",
            "ios = \"difference\"\nios_ipad = \"phone.g\"\nios_iphone = \"phone.g\"",
            "ios = \"difference\"\nios_ipad = \"ipad.g\"",
        ] {
            assert!(!check(&ios(extra)).unwrap().problems.is_empty(), "{extra}");
        }
    }

    #[test]
    fn a_difference_without_a_platform_reason_fails() {
        let text = "[feature.x]\ndescription = \"d\"\nswift = \"done\"\ncompose = \"done\"\nios = \"difference\"\nios_ipad = \"ipad.g\"\nios_iphone = \"phone.g\"\n[ipad.g]\ndescription = \"d\"\n[phone.g]\ndescription = \"d\"\nplatform_reason = \"  \"\n";
        let r = check(text).unwrap();
        let n = r
            .problems
            .iter()
            .filter(|p| p.contains("no platform_reason"))
            .count();
        assert_eq!(n, 2, "{:?}", r.problems);
    }
}
