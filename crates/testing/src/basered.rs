// SPDX-License-Identifier: GPL-3.0-or-later

//! A red the base has too (s246, decision 0241).
//!
//! A task is not charged for a red that the same suite has at its merge base. This module is the
//! pure part: finding the first error in a build's or a run's output, normalising it so that two
//! runs of the same failure compare equal (paths, line and column numbers, hex ids and digits are
//! stripped), and saying whether the base is red *the same way* as the branch.

/// How the branch's suite was red, for the kinds of red that name no failing test.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RedKind {
    /// The suite's build failed.
    Build,
    /// The run proved nothing: too few tests ran.
    Vacuous,
    /// The run was red and no failing test could be named.
    Nameless,
}

impl RedKind {
    pub fn name(self) -> &'static str {
        match self {
            RedKind::Build => "build",
            RedKind::Vacuous => "vacuous",
            RedKind::Nameless => "nameless",
        }
    }

    pub fn parse(s: &str) -> Option<RedKind> {
        [RedKind::Build, RedKind::Vacuous, RedKind::Nameless]
            .into_iter()
            .find(|k| k.name() == s)
    }
}

/// What the suite did at the base, as far as the comparison needs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BaseSide {
    /// The build was red: its first error.
    pub build_error: Option<String>,
    /// The run executed too few tests.
    pub vacuous: bool,
    /// The run was red.
    pub red: bool,
    /// The tests that failed.
    pub failing: Vec<String>,
    /// The first error line of a red run.
    pub first_error: String,
}

impl BaseSide {
    /// Nothing is wrong at the base: the build passed, the run proved something and passed.
    pub fn green(&self) -> bool {
        self.build_error.is_none() && !self.vacuous && !self.red && self.failing.is_empty()
    }
}

/// Is the base red the same way as the branch? `first_error` is the branch's raw first error
/// (empty for a vacuous run, which has none).
pub fn same_red(kind: RedKind, first_error: &str, base: &BaseSide) -> bool {
    match kind {
        RedKind::Build => base
            .build_error
            .as_deref()
            .is_some_and(|b| !normalise(b).is_empty() && normalise(b) == normalise(first_error)),
        // A build that is red at the base also runs nothing there.
        RedKind::Vacuous => base.vacuous,
        RedKind::Nameless => {
            base.build_error.is_none()
                && base.red
                && !base.first_error.is_empty()
                && normalise(&base.first_error) == normalise(first_error)
        }
    }
}

/// The first line of `output` that reports an error; the last non-empty line when none does.
/// Whole-output scans are bounded: a build log can be huge.
pub fn first_error(output: &str) -> String {
    let lines = || output.lines().map(str::trim).filter(|l| !l.is_empty());
    let hit = lines().take(20_000).find(|l| {
        let low = l.to_ascii_lowercase();
        low.starts_with("error")
            || low.contains(" error:")
            || low.contains("error[")
            || low.contains(": error")
            || low.starts_with("e: ")
            || low.starts_with("fatal")
            || low.contains("failure: build failed")
            || low.starts_with("* what went wrong")
            || low.contains("build failed")
            || low.contains("panicked at")
    });
    let line = match hit {
        // Gradle's "What went wrong" is followed by the line that says it.
        Some(l) if l.starts_with("* What went wrong") => {
            let mut it = output.lines().map(str::trim).skip_while(|x| *x != l);
            it.next();
            it.find(|x| !x.is_empty()).unwrap_or(l)
        }
        Some(l) => l,
        None => lines().next_back().unwrap_or(""),
    };
    line.chars().take(300).collect()
}

/// The error with what varies between runs removed: a word holding a `/` becomes `<path>`, a
/// hex run of seven or more or a `0x` number becomes `<hex>`, every other run of digits becomes
/// `<n>`, and spaces are collapsed. Two sightings of the same failure then compare equal.
pub fn normalise(line: &str) -> String {
    let mut words: Vec<String> = vec![];
    for w in line.split_whitespace() {
        let w = w.trim_matches(|c: char| matches!(c, '`' | '\'' | '"' | '(' | ')' | ',' | ';'));
        if w.contains('/') || w.contains('\\') {
            words.push("<path>".into());
            continue;
        }
        words.push(strip_numbers(w));
    }
    let mut out = words.join(" ");
    // `<path>:12:3:` and the like leave a bare colon behind.
    while out.contains("<path>:") {
        out = out.replace("<path>:", "<path>");
    }
    out.chars().take(200).collect()
}

fn strip_numbers(w: &str) -> String {
    let b: Vec<char> = w.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c == '0' && i + 1 < b.len() && (b[i + 1] == 'x' || b[i + 1] == 'X') {
            let mut j = i + 2;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            out.push_str("<hex>");
            i = j;
        } else if c.is_ascii_hexdigit() && !c.is_ascii_alphabetic() | c.is_ascii_alphabetic() {
            // A run of hex digits is an id when it is long enough, a number or a word otherwise.
            let mut j = i;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            let run: String = b[i..j].iter().collect();
            let word_before = i > 0 && b[i - 1].is_alphanumeric();
            let word_after = j < b.len() && (b[j].is_alphanumeric() || b[j] == '_');
            if run.len() >= 7
                && run.chars().any(|c| c.is_ascii_digit())
                && !word_before
                && !word_after
            {
                out.push_str("<hex>");
            } else if run.chars().all(|c| c.is_ascii_digit()) && !word_before {
                out.push_str("<n>");
            } else {
                out.push_str(&run);
            }
            i = j;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// The title of the task filed for a base that is red: the suite, the base and the first error.
pub fn title(suite: &str, base_short: &str, error: &str) -> String {
    format!("`{suite}` is red on {base_short}: {}", title_error(error))
}

fn title_error(error: &str) -> String {
    let e: String = error.split_whitespace().collect::<Vec<_>>().join(" ");
    if e.is_empty() {
        "it runs no tests".into()
    } else {
        e.chars().take(100).collect()
    }
}

/// Is `title` the title of a filing for this suite and this (normalised) error, whatever base it
/// names? A second task that meets the same red finds the first one's filing by this.
pub fn is_title_for(title: &str, suite: &str, error: &str) -> bool {
    let head = format!("`{suite}` is red on ");
    let Some(rest) = title.strip_prefix(&head) else {
        return false;
    };
    let Some((_, e)) = rest.split_once(": ") else {
        return false;
    };
    normalise(e) == normalise(&title_error(error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_numbers_and_ids_do_not_tell_two_errors_apart() {
        let a = "error[E0432]: unresolved import `crate::foo` at /Users/x/wt1/src/lib.rs:12:5";
        let b = "error[E0432]: unresolved import `crate::foo` at /tmp/other/src/lib.rs:99:17";
        assert_eq!(normalise(a), normalise(b));
        assert_ne!(
            normalise(a),
            normalise("error[E0433]: failed to resolve: use of undeclared crate")
        );
        assert_eq!(
            normalise("could not find 0xdeadbeef in cache 3f2a9c1b77"),
            normalise("could not find 0x1234 in cache aa11bb22cc")
        );
        assert_eq!(
            normalise("Execution failed for task ':app:compileKotlin' after 41 s"),
            normalise("Execution failed for task ':app:compileKotlin' after 7 s")
        );
    }

    #[test]
    fn the_first_error_is_found_in_a_build_log() {
        let out = "   Compiling a v0.1.0\n   Compiling b v0.1.0\nerror[E0432]: unresolved import\n --> src/lib.rs:1:5\nerror: could not compile `b`\n";
        assert_eq!(first_error(out), "error[E0432]: unresolved import");
        let gradle = "> Task :app:compileKotlin FAILED\n\nFAILURE: Build failed with an exception.\n\n* What went wrong:\nExecution failed for task ':app:compileKotlin'.\n";
        assert_eq!(
            first_error(gradle),
            "FAILURE: Build failed with an exception."
        );
        assert_eq!(first_error("one\n\ntwo\n"), "two");
        assert_eq!(first_error(""), "");
    }

    #[test]
    fn a_filing_is_found_again_by_suite_and_normalised_error() {
        let t = title(
            "compose-clients",
            "812a7849ab",
            "error: could not compile `x` at /a/b.rs:3:4",
        );
        assert!(t.starts_with("`compose-clients` is red on 812a7849ab: error:"));
        assert!(is_title_for(
            &t,
            "compose-clients",
            "error: could not compile `x` at /z/y.rs:99:1"
        ));
        assert!(!is_title_for(
            &t,
            "swift-clients",
            "error: could not compile `x` at /a/b.rs:3:4"
        ));
        assert!(!is_title_for(&t, "compose-clients", "error: another thing"));
        let v = title("s", "abc", "");
        assert!(v.ends_with("it runs no tests") && is_title_for(&v, "s", ""));
    }

    #[test]
    fn the_base_is_red_the_same_way_or_it_is_not() {
        let build = BaseSide {
            build_error: Some("error[E0432]: unresolved import at /a/b.rs:1:2".into()),
            ..Default::default()
        };
        assert!(same_red(
            RedKind::Build,
            "error[E0432]: unresolved import at /c/d.rs:9:9",
            &build
        ));
        assert!(!same_red(RedKind::Build, "error[E0599]: no method", &build));
        assert!(!same_red(RedKind::Build, "error: x", &BaseSide::default()));
        let vac = BaseSide {
            vacuous: true,
            ..Default::default()
        };
        assert!(same_red(RedKind::Vacuous, "", &vac));
        assert!(!same_red(RedKind::Vacuous, "", &BaseSide::default()));
        let red = BaseSide {
            red: true,
            first_error: "FAILURE: exit 7 at line 12".into(),
            ..Default::default()
        };
        assert!(same_red(
            RedKind::Nameless,
            "FAILURE: exit 3 at line 99",
            &red
        ));
        assert!(!same_red(RedKind::Nameless, "something else", &red));
        assert!(!same_red(
            RedKind::Nameless,
            "",
            &BaseSide {
                red: true,
                ..Default::default()
            }
        ));
        assert!(BaseSide::default().green());
        assert!(!red.green());
    }
}
