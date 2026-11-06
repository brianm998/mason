// SPDX-License-Identifier: GPL-3.0-or-later

//! `clients/features.toml`: every feature and its status in each client. A
//! feature marked done in only one client fails the check unless it has a
//! recorded platform reason; "n/a" always needs one.

use toml_edit::DocumentMut;

pub const CLIENTS: [&str; 2] = ["swift", "compose"];

#[derive(Debug, PartialEq, Eq)]
pub struct Report {
    pub features: usize,
    pub done: [usize; 2],
    pub problems: Vec<String>,
}

pub fn check(text: &str) -> Result<Report, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("not valid TOML: {e}"))?;
    let feats = doc
        .get("feature")
        .and_then(|f| f.as_table_like())
        .ok_or("no [feature.*] tables")?;
    let mut r = Report {
        features: 0,
        done: [0, 0],
        problems: vec![],
    };
    for (name, item) in feats.iter() {
        let t = item
            .as_table_like()
            .ok_or_else(|| format!("feature.{name} must be a table"))?;
        r.features += 1;
        let get = |k: &str| t.get(k).and_then(|v| v.as_str()).unwrap_or("");
        let reason = get("platform_reason").trim();
        let mut status = [""; 2];
        for (i, c) in CLIENTS.iter().enumerate() {
            let s = get(c);
            if !matches!(s, "done" | "todo" | "n/a") {
                r.problems.push(format!(
                    "feature.{name}.{c} is {s:?}; it must be \"done\", \"todo\" or \"n/a\""
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
        let uneven = status[0] != status[1];
        let na = status.contains(&"n/a");
        if uneven && reason.is_empty() {
            r.problems.push(format!(
                "feature.{name}: swift is {:?} and compose is {:?}, with no platform_reason. Finish it in both clients, or record why one cannot have it",
                status[0], status[1]
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
            "[feature.x]\ndescription = \"d\"\nswift = \"{sw}\"\ncompose = \"{co}\"\n{}",
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
        assert_eq!(r.done, [1, 0]);
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
}
