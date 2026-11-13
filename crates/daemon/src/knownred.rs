// SPDX-License-Identifier: GPL-3.0-or-later

//! Known-red tests: the tests that are red for reasons that are not the task's, and what
//! every path that runs tests does with them (s281). Pure logic over a small interface.
//!
//! The interface is [`KnownRed`]: "which tests are known red for this suite or step". Today
//! it is backed by the `quarantine` table ([`QuarantineLedger`]; its `reason` stands for the
//! fix). The known-red ledger of #280 adds a `fix_task` and automatic entries to that same
//! table; it only has to implement [`KnownRed`] (or fill `Known::fix`) to be picked up here.

use mason_store::Store;

/// One test known to be red, and where its fix is tracked (free text such as `task #12`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Known {
    pub test: String,
    pub fix: Option<String>,
}

/// Which tests are known red for `scope` (a suite name, or a test step's name).
pub trait KnownRed {
    fn known(&self, scope: &str) -> Vec<Known>;
}

/// The quarantine table, unexpired entries only.
pub struct QuarantineLedger<'a> {
    pub store: &'a Store,
    pub now_ms: u64,
}

impl KnownRed for QuarantineLedger<'_> {
    fn known(&self, scope: &str) -> Vec<Known> {
        self.store
            .quarantined(self.now_ms)
            .unwrap_or_default()
            .into_iter()
            .filter(|q| q.0 == scope)
            .map(|q| Known {
                test: q.1,
                fix: Some(q.4).filter(|r| !r.trim().is_empty()),
            })
            .collect()
    }
}

/// The ledger of this daemon's store, for `scope`.
pub fn known_for(store: &Store, scope: &str) -> Vec<Known> {
    QuarantineLedger {
        store,
        now_ms: crate::daemon::now_ms(),
    }
    .known(scope)
}

/// Split failing tests into those that are the change's (`new`) and those in the ledger.
pub fn split<'a>(failing: &[String], known: &'a [Known]) -> (Vec<String>, Vec<&'a Known>) {
    let mut new = vec![];
    let mut excused: Vec<&Known> = vec![];
    for f in failing {
        match known.iter().find(|k| &k.test == f) {
            Some(k) => {
                if !excused.iter().any(|e| e.test == k.test) {
                    excused.push(k);
                }
            }
            None => new.push(f.clone()),
        }
    }
    (new, excused)
}

/// A failure is excused by the ledger only when it names at least one test and every one is
/// known. An unreadable red (no names) is never excused.
pub fn all_known(failing: &[String], known: &[Known]) -> bool {
    !failing.is_empty() && split(failing, known).0.is_empty()
}

/// `test: known red, not yours (fix: ...)` for each excused test.
pub fn lines(excused: &[&Known]) -> Vec<String> {
    excused
        .iter()
        .map(|k| match &k.fix {
            Some(f) => format!("{}: known red, not yours (fix: {f})", k.test),
            None => format!("{}: known red, not yours (no fix is tracked)", k.test),
        })
        .collect()
}

/// For `run_check`: a header naming every failing test that is known red, or `None`.
pub fn header(output: &str, known: &[Known]) -> Option<String> {
    let failing = crate::rerun::failing_names(output);
    let (_, excused) = split(&failing, known);
    if excused.is_empty() {
        return None;
    }
    Some(format!(
        "Known red, not yours: do not work on these tests.\n{}",
        lines(&excused)
            .iter()
            .map(|l| format!("  {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The suite's command without `tests`: its `skip` template with `{tests}` (the quoted names)
/// and `{skips}` (`--skip 'a' --skip 'b'`) filled in. `None` when there is nothing to skip.
pub fn skip_command(template: &str, tests: &[String]) -> Option<String> {
    if tests.is_empty() {
        return None;
    }
    let names = tests.iter().map(|t| quote(t)).collect::<Vec<_>>();
    let skips = names
        .iter()
        .map(|n| format!("--skip {n}"))
        .collect::<Vec<_>>();
    Some(
        template
            .replace("{tests}", &names.join(" "))
            .replace("{skips}", &skips.join(" ")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(t: &str, fix: Option<&str>) -> Known {
        Known {
            test: t.into(),
            fix: fix.map(String::from),
        }
    }
    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_known_and_a_new_failure_are_told_apart() {
        let known = [k("a::old", Some("task #7"))];
        let (new, excused) = split(&s(&["a::old", "b::new"]), &known);
        assert_eq!(new, s(&["b::new"]));
        assert_eq!(
            lines(&excused),
            vec!["a::old: known red, not yours (fix: task #7)"]
        );
        assert!(!all_known(&s(&["a::old", "b::new"]), &known));
        assert!(all_known(&s(&["a::old"]), &known));
        assert!(
            !all_known(&[], &known),
            "a red with no names is never excused"
        );
    }

    #[test]
    fn run_check_output_names_the_known_red_test_and_its_fix() {
        let known = [k("a::old", Some("task #7"))];
        let out = "test a::old ... FAILED\ntest b::ok ... ok\n";
        let h = header(out, &known).unwrap();
        assert!(h.contains("a::old") && h.contains("task #7") && h.contains("do not work"));
        assert!(header("test c::x ... FAILED\n", &known).is_none());
    }

    #[test]
    fn the_skip_template_carries_exact_quoted_names() {
        let t = "cargo test -- --exact {skips}";
        assert_eq!(
            skip_command(t, &s(&["a::old", "it's"])).unwrap(),
            "cargo test -- --exact --skip 'a::old' --skip 'it'\\''s'"
        );
        assert_eq!(skip_command("run {tests}", &s(&["a"])).unwrap(), "run 'a'");
        assert!(skip_command(t, &[]).is_none());
    }
}
