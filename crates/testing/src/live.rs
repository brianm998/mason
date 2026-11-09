// SPDX-License-Identifier: GPL-3.0-or-later

//! A live count of tests, for progress only: the verdict still comes from the
//! full [`crate::parse::parse`] of the whole output at the end.
//!
//! [`LiveCount`] is fed bytes as they arrive, cuts them into lines and counts
//! passed, failed and skipped with the same per-line functions the full parse
//! uses. It exists for the formats that are lines (TAP, nextest, pytest,
//! xctest, Swift Testing, Gradle, regex). JUnit, Playwright and Cucumber are
//! whole documents, so [`LiveCount::new`] gives none for them.

use crate::model::{ParserKind, Suite};
use crate::parse::LineParser;
use crate::results::Status;
use std::collections::HashMap;

/// What has been counted so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub passed: u64,
    pub failed: u64,
    pub skipped: u64,
}

impl Counts {
    /// Tests that ran (a skipped test did not).
    pub fn executed(&self) -> u64 {
        self.passed + self.failed
    }
}

/// 0 pass, 1 skip, 2 fail: a failure beats the rest, as in the full parse.
fn rank(s: &Status) -> u8 {
    match s {
        Status::Pass => 0,
        Status::Skip => 1,
        Status::Fail(_) => 2,
    }
}

pub struct LiveCount {
    parser: LineParser,
    tail: Vec<u8>,
    counts: Counts,
    /// For the parsers where a repeated name is one test: the first status's
    /// slot, upgraded to a failure when one arrives.
    seen: HashMap<String, Status>,
}

impl LiveCount {
    /// A counter for this suite's parser, or None when its output is a
    /// whole document (or the regex parser has no usable patterns).
    pub fn new(kind: ParserKind, suite: &Suite) -> Option<LiveCount> {
        Some(LiveCount {
            parser: LineParser::new(kind, suite)?,
            tail: vec![],
            counts: Counts::default(),
            seen: HashMap::new(),
        })
    }

    /// More output.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.tail.extend_from_slice(bytes);
        while let Some(i) = self.tail.iter().position(|b| *b == b'\n') {
            let raw: Vec<u8> = self.tail.drain(..=i).collect();
            let mut line = &raw[..raw.len() - 1];
            if line.last() == Some(&b'\r') {
                line = &line[..line.len() - 1];
            }
            let text = String::from_utf8_lossy(line).into_owned();
            self.line(&text);
        }
    }

    /// The output is over: an unfinished last line counts as a line.
    pub fn finish(&mut self) {
        if !self.tail.is_empty() {
            let raw = std::mem::take(&mut self.tail);
            let text = String::from_utf8_lossy(&raw).into_owned();
            self.line(&text);
        }
    }

    pub fn counts(&self) -> Counts {
        self.counts
    }

    fn bump(&mut self, s: &Status, by: i64) {
        let slot = match s {
            Status::Pass => &mut self.counts.passed,
            Status::Skip => &mut self.counts.skipped,
            Status::Fail(_) => &mut self.counts.failed,
        };
        *slot = (*slot as i64 + by).max(0) as u64;
    }

    fn line(&mut self, l: &str) {
        let Some(case) = self.parser.line(l) else {
            return;
        };
        if !self.parser.dedupes() {
            self.bump(&case.status, 1);
            return;
        }
        match self.seen.get(&case.name) {
            None => {
                self.bump(&case.status, 1);
                self.seen.insert(case.name, case.status);
            }
            Some(old) => {
                // Same rule as the full parse: only a failure replaces what was seen.
                if matches!(case.status, Status::Fail(_)) && rank(old) < 2 {
                    let old = old.clone();
                    self.bump(&old, -1);
                    self.bump(&case.status, 1);
                    self.seen.insert(case.name, case.status);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse;

    fn suite(kind: ParserKind) -> Suite {
        let mut s = Suite::new("t", "true", kind);
        s.pass_regex = Some(r"^PASS (\w+)".into());
        s.fail_regex = Some(r"^FAIL (\w+)".into());
        s
    }

    fn fixtures() -> Vec<(ParserKind, &'static str)> {
        vec![
            (
                ParserKind::Tap,
                "TAP version 13\n1..5\nok 1 - a\nnot ok 2 - b\nok 3 - c # SKIP no\nok 4\nok 5 - e # todo\n",
            ),
            (
                ParserKind::Xctest,
                "Test Case 'A.a' started.\nTest Case 'A.a' passed (0.1 seconds).\nTest Case 'A.b' failed (0.1 seconds).\nTest Case 'A.c' skipped (0.0 seconds).\n",
            ),
            (
                ParserKind::SwiftTesting,
                "x Test one passed after 0.1 seconds.\nx Test two failed after 0.1 seconds.\nx Test three skipped\nx Test run with 3 tests passed after 1s\n",
            ),
            (
                ParserKind::Nextest,
                "        PASS [   0.01s] c t::a\n        FAIL [   0.01s] c t::b\n        FAIL [   0.01s] c t::a\n        PASS [   0.01s] c t::b\n        SKIP [   0.00s] c t::c\n        PASS [   0.01s] c t::d\n",
            ),
            (
                ParserKind::Pytest,
                "t.py::a PASSED [ 10%]\nt.py::b FAILED [ 20%]\nt.py::c SKIPPED [ 30%]\n[gw0] [ 40%] PASSED t.py::d\n[gw1] [ 50%] FAILED t.py::d\nt.py::b PASSED\n",
            ),
            (
                ParserKind::Gradle,
                "FooTest > a() PASSED\nFooTest > b() FAILED\nFooTest > c() SKIPPED\nBUILD done\n",
            ),
            (
                ParserKind::Regex,
                "PASS a\nFAIL b\nnoise\nPASS c\n",
            ),
        ]
    }

    fn full(kind: ParserKind, text: &str) -> Counts {
        let r = parse(kind, text, &suite(kind)).unwrap();
        Counts {
            passed: r.passed() as u64,
            failed: r.failures().len() as u64,
            skipped: r.skipped() as u64,
        }
    }

    fn live(kind: ParserKind, chunks: &[&[u8]]) -> Counts {
        let mut c = LiveCount::new(kind, &suite(kind)).unwrap();
        for ch in chunks {
            c.feed(ch);
        }
        c.finish();
        c.counts()
    }

    #[test]
    fn live_count_equals_full_parse() {
        for (kind, text) in fixtures() {
            let want = full(kind, text);
            assert!(want.passed + want.failed + want.skipped > 0, "{kind:?}");
            assert_eq!(live(kind, &[text.as_bytes()]), want, "{kind:?} whole");
            // The same with CRLF line ends.
            let crlf = text.replace('\n', "\r\n");
            assert_eq!(
                live(kind, &[crlf.as_bytes()]),
                full(kind, &crlf),
                "{kind:?} crlf"
            );
            // And without the final newline.
            let bare = text.trim_end_matches('\n');
            assert_eq!(
                live(kind, &[bare.as_bytes()]),
                full(kind, bare),
                "{kind:?} bare"
            );
            // One byte at a time.
            let bytes = text.as_bytes();
            let ones: Vec<&[u8]> = bytes.chunks(1).collect();
            assert_eq!(live(kind, &ones), want, "{kind:?} bytes");
            // Split at every position.
            for i in 0..=bytes.len() {
                assert_eq!(
                    live(kind, &[&bytes[..i], &bytes[i..]]),
                    want,
                    "{kind:?} split {i}"
                );
            }
        }
    }

    #[test]
    fn counts_rise_as_lines_arrive() {
        let mut c = LiveCount::new(ParserKind::Tap, &suite(ParserKind::Tap)).unwrap();
        c.feed(b"ok 1 - a\nok 2 - b");
        assert_eq!(c.counts().passed, 1, "an unfinished line waits");
        c.feed(b"\nok 3 - c\n");
        assert_eq!(c.counts().passed, 3);
    }

    #[test]
    fn whole_document_formats_have_no_live_count() {
        for k in [
            ParserKind::Junit,
            ParserKind::Playwright,
            ParserKind::Cucumber,
        ] {
            assert!(LiveCount::new(k, &suite(k)).is_none());
        }
    }
}
