// SPDX-License-Identifier: GPL-3.0-or-later

//! Timing-dependent tests, refused at submit (s252, decision 0247).
//!
//! A test that is right or wrong according to how fast the machine is turns red when the
//! machine is busy, which it nearly always is. The weakening scan reads the ADDED lines of
//! test code and names the ones that
//!
//! * sleep a fixed time and then assert within the next few lines (`thread::sleep`,
//!   `Thread.sleep`, `delay(`, `sleep(`, `usleep(`);
//! * assert on `elapsed()` or a difference of `Instant`s against a constant;
//! * use a fixed sub-second timeout as synchronisation (`recv_timeout`, `wait_timeout`,
//!   `park_timeout`; Swift `wait(for:timeout:)` under one second).
//!
//! Waiting in a loop for a condition under a generous deadline is the sanctioned pattern and is
//! not flagged. A line, or the one above it, that carries
//! `// timing: owner <name> expires <YYYY-MM-DD>` with a date not yet past is let through.
//! What this does not catch is written in decision 0247.

use super::Finding;

/// `(year, month, day)` of today, UTC.
pub fn today() -> (i64, u32, u32) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    civil(secs.div_euclid(86_400))
}

/// Days since 1970-01-01 as a civil date (Howard Hinnant's algorithm).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    Rust,
    Kotlin,
    Swift,
}

/// The language of a file when it is test code by its path; Rust elsewhere is test code only
/// after a `#[test]` or `#[cfg(test)]` line in the added text.
fn lang_of(file: &str) -> Option<(Lang, bool)> {
    if file.ends_with(".rs") {
        let by_path = file.starts_with("tests/") || file.contains("/tests/");
        return Some((Lang::Rust, by_path));
    }
    if file.ends_with(".kt") && (file.contains("src/test/") || file.contains("/test/")) {
        return Some((Lang::Kotlin, true));
    }
    if file.ends_with(".swift") && file.contains("Tests/") {
        return Some((Lang::Swift, true));
    }
    None
}

const ASSERT_WORDS: &[&str] = &[
    "assert!(",
    "assert_eq!(",
    "assert_ne!(",
    "assert(",
    "assertEquals(",
    "assertTrue(",
    "assertFalse(",
    "assertNull(",
    "assertNotNull(",
    "assertThat(",
    "XCTAssert",
    "#expect(",
    "expect(",
];

/// How far past a sleep an assertion still counts as "right after".
const NEAR: usize = 5;
/// How far back a loop opener makes a sleep a poll.
const LOOP_BACK: usize = 10;

/// The line with the text of its string literals removed: a sleep or an assertion that is only
/// text (test data, a message, a shell command) is not code that runs.
fn without_strings(t: &str) -> String {
    let mut out = String::new();
    let mut in_str = false;
    let mut esc = false;
    for c in t.chars() {
        if in_str {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                in_str = false;
                out.push('"');
            }
        } else {
            if c == '"' {
                in_str = true;
            }
            out.push(c);
        }
    }
    out
}

/// A deadline check: an elapsed-time assertion whose message says it is a timeout. Waiting for
/// a condition under a deadline is the sanctioned pattern.
fn is_deadline_check(t: &str) -> bool {
    let low = t.to_ascii_lowercase();
    (low.contains("elapsed") || low.contains("instant::now"))
        && ["timed out", "timeout", "deadline", "gave up"]
            .iter()
            .any(|w| low.contains(w))
}

fn is_comment(t: &str) -> bool {
    t.starts_with("//") || t.starts_with('*') || t.starts_with("/*")
}

fn has_assert(t: &str) -> bool {
    !is_comment(t) && ASSERT_WORDS.iter().any(|a| t.contains(a))
}

fn opens_loop(t: &str) -> bool {
    let t = t.trim_start();
    t.starts_with("while ")
        || t.starts_with("while(")
        || t.starts_with("loop {")
        || t.starts_with("loop{")
        || t.starts_with("for ")
        || t.starts_with("repeat(")
        || t.starts_with("repeat {")
        || t.contains("wait_for(")
        || t.contains("until")
        || t.contains(".forEach")
}

/// The number in `name(` when it is a plain integer literal.
fn int_after(t: &str, name: &str) -> Option<u64> {
    let i = t.find(name)? + name.len();
    let digits: String = t[i..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '_')
        .filter(|c| *c != '_')
        .collect();
    digits.parse().ok()
}

/// Is `name(` called here, as its own word (not the end of a longer name)?
fn calls(t: &str, name: &str) -> bool {
    let pat = format!("{name}(");
    t.match_indices(&pat).any(|(i, _)| {
        t[..i]
            .chars()
            .next_back()
            .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    })
}

/// A fixed duration written in the line, in milliseconds (`None`: no literal duration).
fn fixed_millis(t: &str) -> Option<u64> {
    if let Some(n) = int_after(t, "from_millis(") {
        return Some(n);
    }
    if let Some(n) = int_after(t, "from_secs(") {
        return Some(n.saturating_mul(1000));
    }
    if let Some(n) = int_after(t, "from_micros(") {
        return Some(n / 1000);
    }
    if let Some(n) = int_after(t, "from_nanos(") {
        return Some(n / 1_000_000);
    }
    // `from_secs_f64(0.5)`: a leading zero is sub-second.
    let inner = t.split_once("from_secs_f")?.1.split_once('(')?.1;
    let whole: String = inner.chars().take_while(|c| c.is_ascii_digit()).collect();
    match whole.parse::<u64>().ok()? {
        0 => Some(500),
        n => Some(n.saturating_mul(1000)),
    }
}

/// A pause of 10 ms or less between two actions, to give them different clock readings: it
/// waits for nothing, and a slow machine cannot make it too short.
fn separates_timestamps(t: &str) -> bool {
    t.contains("sleep(") && fixed_millis(t).is_some_and(|ms| ms <= 10)
}

/// A sleep of a fixed length.
fn is_fixed_sleep(lang: Lang, t: &str) -> bool {
    if is_comment(t) {
        return false;
    }
    match lang {
        Lang::Rust => {
            (calls(t, "sleep") || t.contains("thread::sleep(")) && fixed_millis(t).is_some()
        }
        Lang::Kotlin => t.contains("Thread.sleep(") || calls(t, "delay"),
        Lang::Swift => calls(t, "sleep") || calls(t, "usleep"),
    }
}

/// A fixed sub-second timeout used to wait for something.
fn is_short_timeout(lang: Lang, t: &str) -> Option<&'static str> {
    if is_comment(t) {
        return None;
    }
    match lang {
        Lang::Rust => {
            let sync = [
                "recv_timeout(",
                "wait_timeout(",
                "wait_timeout_while(",
                "park_timeout(",
            ];
            if sync.iter().any(|s| t.contains(s)) {
                if let Some(ms) = fixed_millis(t) {
                    if ms < 1000 {
                        return Some("a sub-second fixed timeout as synchronisation");
                    }
                }
            }
            None
        }
        Lang::Swift => {
            if t.contains("wait(for:") || t.contains(".wait(for") {
                let ms = t
                    .split("timeout:")
                    .nth(1)
                    .map(|r| r.trim_start())
                    .and_then(|r| {
                        let n: String = r
                            .chars()
                            .take_while(|c| c.is_ascii_digit() || *c == '.')
                            .collect();
                        n.parse::<f64>().ok()
                    });
                if ms.is_some_and(|s| s < 1.0) {
                    return Some("a sub-second wait(for:timeout:)");
                }
            }
            None
        }
        Lang::Kotlin => None,
    }
}

fn asserts_on_elapsed(t: &str) -> bool {
    if !has_assert(t) {
        return false;
    }
    let timed = t.contains("elapsed()") || t.contains("duration_since(") || t.contains("Instant");
    let cmp = t.contains('<') || t.contains('>');
    let constant = t.contains("Duration::from_")
        || t.chars().any(|c| c.is_ascii_digit()) && t.contains("elapsed");
    timed && cmp && constant
}

/// The date in a valid marker on this line or the one above, if any: `Some(true)` when it has
/// not passed, `Some(false)` when it has, `None` when there is no marker.
fn marker(lines: &[String], i: usize, today: (i64, u32, u32)) -> Option<bool> {
    let check = |l: &str| -> Option<bool> {
        let rest = l.split_once("// timing: owner ")?.1;
        let (name, tail) = rest.split_once(" expires ")?;
        if name.trim().is_empty() {
            return None;
        }
        let date: String = tail.trim().chars().take(10).collect();
        let mut p = date.split('-');
        let (y, m, d) = (
            p.next()?.parse::<i64>().ok()?,
            p.next()?.parse::<u32>().ok()?,
            p.next()?.parse::<u32>().ok()?,
        );
        if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
            return None;
        }
        Some((y, m, d) >= today)
    };
    check(&lines[i]).or_else(|| i.checked_sub(1).and_then(|j| check(&lines[j])))
}

/// The timing-dependent lines among the added lines of one file. `at` is each added line's
/// number in the new file; lines are "near" each other only when their numbers are consecutive.
pub fn findings(
    file: &str,
    added: &[String],
    at: &[usize],
    today: (i64, u32, u32),
) -> Vec<Finding> {
    let Some((lang, by_path)) = lang_of(file) else {
        return vec![];
    };
    // Rust test code outside `tests/`: from the first added `#[test]` / `#[cfg(test)]` on.
    let from = if by_path {
        Some(0)
    } else {
        added.iter().position(|l| {
            let t = l.trim();
            t == "#[test]" || t.starts_with("#[cfg(test)]") || t.starts_with("mod tests")
        })
    };
    let Some(from) = from else {
        return vec![];
    };
    let near = |i: usize, j: usize| j < added.len() && at[j] == at[i] + (j - i);
    let in_loop = |i: usize| {
        (1..=LOOP_BACK).any(|k| {
            i.checked_sub(k)
                .is_some_and(|j| at[j] + k == at[i] && opens_loop(&added[j]))
        })
    };
    // A deadline check just above a sleep makes the sleep a poll.
    let polls = |i: usize| {
        (1..=3).any(|k| {
            i.checked_sub(k)
                .is_some_and(|j| at[j] + k == at[i] && is_deadline_check(&added[j]))
        })
    };
    let mut out = vec![];
    for i in from..added.len() {
        let original = added[i].trim();
        let stripped = without_strings(original);
        let t = stripped.trim();
        let mut rule: Option<&'static str> = None;
        if is_fixed_sleep(lang, t) && !in_loop(i) && !polls(i) && !separates_timestamps(t) {
            let swift = lang == Lang::Swift;
            // An assertion counts only in the same block: a closing brace ends the search.
            let assert_after = (1..=NEAR)
                .take_while(|k| near(i, i + k) && !added[i + k].trim_start().starts_with('}'))
                .any(|k| has_assert(&without_strings(added[i + k].trim())));
            if swift || assert_after {
                rule = Some(if swift {
                    "a fixed sleep in a test"
                } else {
                    "sleeps a fixed time, then asserts"
                });
            }
        }
        if rule.is_none() && asserts_on_elapsed(t) && !is_deadline_check(original) {
            rule = Some("asserts on elapsed time against a constant");
        }
        if rule.is_none() && !in_loop(i) {
            rule = is_short_timeout(lang, t);
        }
        let Some(rule) = rule else { continue };
        match marker(added, i, today) {
            Some(true) => continue,
            Some(false) => out.push(Finding::new(
                file,
                "timing-dependent-test",
                format!("{rule}; the `// timing:` marker has expired"),
                at[i],
            )),
            None => out.push(Finding::new(
                file,
                "timing-dependent-test",
                format!(
                    "{rule}. Wait for a condition under a generous deadline instead, or mark the line `// timing: owner <name> expires <YYYY-MM-DD>`"
                ),
                at[i],
            )),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TODAY: (i64, u32, u32) = (2026, 11, 13);

    fn scan(file: &str, text: &str) -> Vec<(usize, String)> {
        let added: Vec<String> = text.lines().map(String::from).collect();
        let at: Vec<usize> = (1..=added.len()).collect();
        findings(file, &added, &at, TODAY)
            .into_iter()
            .map(|f| (f.line.unwrap(), f.detail))
            .collect()
    }

    #[test]
    fn the_calendar_is_right() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_040), (2024, 11, 13));
        assert_eq!(civil(20_087), (2024, 12, 30));
        assert_eq!(civil(19_723), (2024, 1, 1));
    }

    #[test]
    fn a_fixed_sleep_followed_by_an_assert_is_flagged() {
        let r = scan(
            "crates/x/tests/t.rs",
            "let h = start();\nstd::thread::sleep(Duration::from_millis(200));\nassert!(h.done());\n",
        );
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].0, 2);
        assert!(r[0].1.contains("sleeps a fixed time"), "{r:?}");
        // No assertion near: only a pause, not flagged.
        assert!(scan(
            "a/tests/t.rs",
            "std::thread::sleep(Duration::from_millis(5));\nlet x = 1;\n"
        )
        .is_empty());
    }

    #[test]
    fn waiting_in_a_loop_for_a_condition_is_the_sanctioned_pattern() {
        let polled = "let end = Instant::now() + Duration::from_secs(900);\nwhile !done() {\n    assert!(Instant::now() < end, \"timed out\");\n    std::thread::sleep(Duration::from_millis(50));\n}\nassert!(done());\n";
        assert!(
            scan("a/tests/t.rs", polled).is_empty(),
            "{:?}",
            scan("a/tests/t.rs", polled)
        );
        let looped = "for _ in 0..500 {\n    if ok() { break; }\n    std::thread::sleep(Duration::from_millis(10));\n}\nassert!(ok());\n";
        assert!(scan("a/tests/t.rs", looped).is_empty());
    }

    #[test]
    fn an_assertion_on_elapsed_time_against_a_constant_is_flagged() {
        let r = scan(
            "a/tests/t.rs",
            "let t = Instant::now();\nwork();\nassert!(t.elapsed() < Duration::from_secs(1));\n",
        );
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].1.contains("elapsed"), "{r:?}");
        // Elapsed time that is only printed is fine.
        assert!(scan("a/tests/t.rs", "println!(\"{:?}\", t.elapsed());\n").is_empty());
    }

    #[test]
    fn a_sub_second_fixed_timeout_as_synchronisation_is_flagged_and_a_long_one_is_not() {
        let r = scan(
            "a/tests/t.rs",
            "let m = rx.recv_timeout(Duration::from_millis(100)).unwrap();\n",
        );
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].1.contains("sub-second"), "{r:?}");
        assert!(scan(
            "a/tests/t.rs",
            "let m = rx.recv_timeout(Duration::from_secs(900)).unwrap();\n"
        )
        .is_empty());
        assert!(scan(
            "a/tests/t.rs",
            "let m = rx.recv_timeout(Duration::from_millis(5000)).unwrap();\n"
        )
        .is_empty());
        assert_eq!(
            scan(
                "a/tests/t.rs",
                "cv.wait_timeout(g, Duration::from_millis(50)).unwrap();\n"
            )
            .len(),
            1
        );
    }

    #[test]
    fn kotlin_and_swift_patterns() {
        let k = scan(
            "clients/compose/app/src/test/kotlin/A.kt",
            "Thread.sleep(300)\nassertEquals(1, x)\n",
        );
        assert_eq!(k.len(), 1, "{k:?}");
        let k2 = scan(
            "clients/compose/app/src/test/kotlin/A.kt",
            "delay(100)\nassertTrue(ok)\n",
        );
        assert_eq!(k2.len(), 1, "{k2:?}");
        assert!(scan(
            "clients/compose/app/src/test/kotlin/A.kt",
            "val x = compute()\nassertEquals(1, x)\n"
        )
        .is_empty());
        let s = scan(
            "clients/swift/Tests/AppTests/A.swift",
            "usleep(100_000)\nXCTAssertTrue(ok)\n",
        );
        assert_eq!(s.len(), 1, "{s:?}");
        let w = scan(
            "clients/swift/Tests/AppTests/A.swift",
            "wait(for: [e], timeout: 0.5)\n",
        );
        assert_eq!(w.len(), 1, "{w:?}");
        assert!(scan(
            "clients/swift/Tests/AppTests/A.swift",
            "wait(for: [e], timeout: 30)\n"
        )
        .is_empty());
        // Not test code by path: left alone.
        assert!(scan("clients/swift/Sources/App/A.swift", "usleep(100)\n").is_empty());
        assert!(scan(
            "crates/x/src/lib.rs",
            "std::thread::sleep(Duration::from_millis(10));\nassert!(x);\n"
        )
        .is_empty());
    }

    #[test]
    fn test_code_inside_a_source_file_counts_from_its_test_attribute() {
        let r = scan(
            "crates/x/src/lib.rs",
            "fn real() {\n    std::thread::sleep(Duration::from_millis(200));\n    assert!(x);\n}\n#[test]\nfn t() {\n    std::thread::sleep(Duration::from_millis(200));\n    assert!(x);\n}\n",
        );
        assert_eq!(r.len(), 1, "{r:?}");
        assert_eq!(r[0].0, 7);
    }

    #[test]
    fn a_marker_with_a_date_not_yet_past_lets_the_line_through_and_an_expired_one_does_not() {
        let line = "std::thread::sleep(Duration::from_millis(200));\nassert!(x);\n";
        // On the line above.
        let above = format!("// timing: owner brian expires 2026-12-01\n{line}");
        assert!(scan("a/tests/t.rs", &above).is_empty());
        // On the line itself, today still counts.
        let same = "std::thread::sleep(Duration::from_millis(200)); // timing: owner brian expires 2026-11-13\nassert!(x);\n";
        assert!(scan("a/tests/t.rs", same).is_empty());
        let expired = format!("// timing: owner brian expires 2026-11-12\n{line}");
        let r = scan("a/tests/t.rs", &expired);
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].1.contains("expired"), "{r:?}");
        // No owner, or a bad date: no marker.
        let bad = format!("// timing: owner  expires 2026-12-01\n{line}");
        assert_eq!(scan("a/tests/t.rs", &bad).len(), 1);
        let bad2 = format!("// timing: owner brian expires soon\n{line}");
        assert_eq!(scan("a/tests/t.rs", &bad2).len(), 1);
    }

    #[test]
    fn what_the_probe_over_the_whole_repository_found_to_be_false_positives_is_not_flagged() {
        // Text that only looks like code (test data in a string).
        assert!(scan(
            "a/tests/t.rs",
            "let src = \"std::thread::sleep(Duration::from_millis(200));\\nassert!(x);\";\n"
        )
        .is_empty());
        // A deadline check beside a poll sleep, outside any loop opener in view.
        let poll = "assert!(started.elapsed() < Duration::from_secs(150), \"timed out\");\nstd::thread::sleep(Duration::from_millis(5));\nassert!(x);\n";
        assert!(
            scan("a/tests/t.rs", poll).is_empty(),
            "{:?}",
            scan("a/tests/t.rs", poll)
        );
        // A pause of 10 ms or less only separates two clock readings.
        assert!(scan("a/tests/t.rs", "emit(a);\nstd::thread::sleep(Duration::from_millis(5));\nemit(b);\nassert!(order());\n").is_empty());
        assert_eq!(
            scan(
                "a/tests/t.rs",
                "std::thread::sleep(Duration::from_millis(20));\nassert!(x);\n"
            )
            .len(),
            1
        );
        // An assertion after the block that holds the sleep closes is not about it.
        assert!(scan("a/tests/t.rs", "let t = spawn(move || {\n    std::thread::sleep(Duration::from_secs(3));\n});\nassert!(x);\n").is_empty());
    }
}
