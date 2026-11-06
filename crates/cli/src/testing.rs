// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason test parse`: read a results file the way a suite would and say what
//! the verdict is. Handy for checking a parser against a real tool's output
//! before writing it into a definition.

use mason_testing::parse::parse as read;
use mason_testing::results::Status;
use mason_testing::verdict::{judge, Verdict};
use mason_testing::{ParserKind, Suite};
use std::path::Path;

/// Prints the cases and the verdict; exit status 0 only for a green.
pub fn parse(kind: &str, file: &Path, min: u32) -> i32 {
    let Some(k) = ParserKind::from_name(kind) else {
        eprintln!(
            "error: unknown parser `{kind}`; the built-in parsers are {}",
            ParserKind::ALL.map(ParserKind::name).join(", ")
        );
        return 2;
    };
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: {}: {e}", file.display());
            return 2;
        }
    };
    let mut suite = Suite::new("file", "", k);
    if k == ParserKind::Regex {
        eprintln!("error: the regex parser needs patterns; set them in a definition's suite");
        return 2;
    }
    suite.min_tests = min;
    let results = match read(k, &text, &suite) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return 2;
        }
    };
    for c in &results.cases {
        let mark = match &c.status {
            Status::Pass => "pass",
            Status::Fail(_) => "FAIL",
            Status::Skip => "skip",
        };
        println!("  {mark}  {}", c.name);
    }
    let v = judge(&results, min, None);
    println!(
        "{} executed, {} passed, {} skipped, {} failed",
        results.executed(),
        results.passed(),
        results.skipped(),
        results.failures().len()
    );
    match v {
        Verdict::Green => {
            println!("green, with proof");
            0
        }
        Verdict::Vacuous { executed, needed } => {
            println!("RED: proved nothing ({executed} test(s) ran, at least {needed} must)");
            1
        }
        Verdict::Red { why, .. } => {
            println!("RED: {why}");
            1
        }
    }
}
