// SPDX-License-Identifier: GPL-3.0-or-later
//! The run time of each cargo test executable, and what it says about a narrowed test step
//! (s649, decision 0642): a step whose selected targets are all short takes the `test-small`
//! lane instead of queueing for `test-suite` behind a whole run.

use std::collections::BTreeMap;

/// The key of a test executable from a `Running ... (target/debug/deps/name-hash)` line: the
/// file name without cargo's hash, `mason_daemon` or `it`. A doc-test run is `doc:<crate>`.
fn exe_key(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("Doc-tests ") {
        return Some(format!("doc:{}", rest.trim().replace('-', "_")));
    }
    let rest = t.strip_prefix("Running ")?;
    let path = rest.rsplit_once('(')?.1.strip_suffix(')')?;
    let file = path.rsplit('/').next()?;
    let (name, hash) = file.rsplit_once('-')?;
    (!name.is_empty() && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| name.to_string())
}

/// `finished in 1.23s` of a `test result:` line, in milliseconds.
fn finished_ms(line: &str) -> Option<u64> {
    let t = line.trim();
    if !t.starts_with("test result:") {
        return None;
    }
    let secs = t.rsplit_once("finished in ")?.1.trim().strip_suffix('s')?;
    let v: f64 = secs.parse().ok()?;
    (v.is_finite() && v >= 0.0).then(|| (v * 1000.0).round() as u64)
}

/// One `(target, ms)` per executable a cargo log ran. Two executables with one key (a crate's
/// library and binary unit tests) are added up.
pub fn parse_log(log: &str) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = vec![];
    let mut current: Option<String> = None;
    for line in log.lines() {
        if let Some(k) = exe_key(line) {
            current = Some(k);
        } else if let Some(ms) = finished_ms(line) {
            let Some(k) = current.take() else { continue };
            match out.iter_mut().find(|(n, _)| *n == k) {
                Some(e) => e.1 += ms,
                None => out.push((k, ms)),
            }
        }
    }
    out
}

/// The executable keys a group of `cargo test` arguments (`-p pkg --lib --test it`) runs, or
/// `None` when it selects something this cannot name (a whole package, `--workspace`).
fn group_keys(group: &str) -> Option<Vec<String>> {
    let mut toks = group.split_whitespace();
    let mut pkg: Option<String> = None;
    let mut keys = vec![];
    let mut kinds: Vec<&str> = vec![];
    let mut tests: Vec<String> = vec![];
    while let Some(t) = toks.next() {
        match t {
            "-p" => pkg = Some(toks.next()?.replace('-', "_")),
            "--lib" | "--bins" | "--doc" => kinds.push(t),
            "--test" => tests.push(toks.next()?.to_string()),
            _ => return None,
        }
    }
    let pkg = pkg?;
    if kinds.is_empty() && tests.is_empty() {
        return None;
    }
    for k in kinds {
        keys.push(if k == "--doc" {
            format!("doc:{pkg}")
        } else {
            pkg.clone()
        });
    }
    keys.extend(tests);
    keys.sort();
    keys.dedup();
    Some(keys)
}

/// The summed median run time, in ms, of the targets `groups` select; `None` when any has no
/// record (it counts as heavy) or a group names something unknown.
pub fn estimate_ms(groups: &[String], medians: &BTreeMap<String, u64>) -> Option<u64> {
    let mut keys: Vec<String> = vec![];
    for g in groups {
        keys.extend(group_keys(g)?);
    }
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        return None;
    }
    keys.iter()
        .try_fold(0u64, |a, k| Some(a.saturating_add(*medians.get(k)?)))
}

/// The estimate, if the narrowed run is under `small_run_s` seconds (0: never small).
pub fn small_estimate_ms(
    groups: &[String],
    medians: &BTreeMap<String, u64>,
    small_run_s: u64,
) -> Option<u64> {
    let ms = estimate_ms(groups, medians)?;
    (small_run_s > 0 && ms < small_run_s.saturating_mul(1000)).then_some(ms)
}

/// The wait text's prefix for a small run.
pub fn label(ms: u64) -> String {
    format!("small run, est. {} s", (ms + 500) / 1000)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOG: &str = "\
   Compiling mason-core v0.1.0
    Finished `test` profile [unoptimized + debuginfo] target(s) in 3.2s
     Running unittests src/lib.rs (target/debug/deps/mason_core-0123abcd4567ef89)

running 2 tests
test a ... ok <0.001s>
test b ... ok <0.002s>

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s

     Running tests/it/main.rs (target/debug/deps/it-89abcdef01234567)

running 1 test
test c ... ok <1.0s>

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 536.50s

   Doc-tests mason-core

running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
";

    fn med(items: &[(&str, u64)]) -> BTreeMap<String, u64> {
        items.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn one_entry_per_running_executable() {
        assert_eq!(
            parse_log(LOG),
            vec![
                ("mason_core".to_string(), 20),
                ("it".to_string(), 536_500),
                ("doc:mason_core".to_string(), 0),
            ]
        );
    }

    #[test]
    fn short_selected_targets_are_small() {
        let m = med(&[("mason_core", 20_000), ("other", 30_000), ("it", 536_000)]);
        let g = vec!["-p mason-core --lib".to_string(), "-p x --test other".to_string()];
        assert_eq!(small_estimate_ms(&g, &m, 180), Some(50_000));
        assert_eq!(label(70_000), "small run, est. 70 s");
    }

    #[test]
    fn a_long_unrecorded_or_off_selection_is_not_small() {
        let m = med(&[("mason_core", 20_000), ("it", 536_000)]);
        let long = vec!["-p mason-core --lib".to_string(), "-p d --test it".to_string()];
        assert_eq!(small_estimate_ms(&long, &m, 180), None);
        let unknown = vec!["-p mason-core --lib".to_string(), "-p d --test new".to_string()];
        assert_eq!(small_estimate_ms(&unknown, &m, 180), None);
        let pkgs = vec!["-p mason-core".to_string()];
        assert_eq!(small_estimate_ms(&pkgs, &m, 180), None);
        let ok = vec!["-p mason-core --lib".to_string()];
        assert_eq!(small_estimate_ms(&ok, &m, 0), None);
        assert_eq!(small_estimate_ms(&[], &m, 180), None);
    }
}
