// SPDX-License-Identifier: GPL-3.0-or-later

//! A suite split across several GitHub runners (#701, decision 0722).
//!
//! The daemon decides which test runs on which shard ([`assign`]), tells each runner through
//! `.mason/run.json`, and joins what the runners send back ([`merge`]) so the suite's own parser
//! and `min_tests` judge one output. Nothing here does I/O.
//!
//! Shards are numbered from 1 in everything a person or a runner sees (`MASON_SHARD`, "shard 3
//! ran 0 of 140 tests"); lists are indexed from 0.

use std::collections::{BTreeMap, BTreeSet};

/// The shard (0-based) a test belongs to when no list names it: FNV-1a over its name's bytes.
/// `scripts/test-shard.py` computes the same, so a test new since the weights runs exactly once.
pub fn hash_shard(name: &str, shards: u32) -> u32 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in name.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (h % u64::from(shards.max(1))) as u32
}

fn median(weights: &BTreeMap<String, u64>) -> u64 {
    let mut v: Vec<u64> = weights.values().copied().collect();
    if v.is_empty() {
        return 1;
    }
    v.sort_unstable();
    v[v.len() / 2].max(1)
}

/// Which tests run on which shard: greedy longest-first onto the shard with the least expected
/// time. `names` are the tests known to exist and `weights` their recorded times in ms; a test
/// with no time takes the median. The same inputs give the same lists (ties go to the lowest
/// shard, equal weights to the name that sorts first), each name appears in exactly one list,
/// and each list is sorted.
pub fn assign(
    names: &BTreeSet<String>,
    weights: &BTreeMap<String, u64>,
    shards: u32,
) -> Vec<Vec<String>> {
    let n = shards.max(1) as usize;
    let unknown = median(weights);
    let mut all: Vec<(u64, &String)> = names
        .iter()
        .map(|t| (weights.get(t).copied().unwrap_or(unknown).max(1), t))
        .collect();
    all.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    let mut load = vec![0u64; n];
    let mut lists: Vec<Vec<String>> = vec![vec![]; n];
    for (w, t) in all {
        let i = (0..n).min_by_key(|i| (load[*i], *i)).unwrap_or(0);
        load[i] += w;
        lists[i].push(t.clone());
    }
    for l in &mut lists {
        l.sort();
    }
    lists
}

/// The shard (0-based) that runs `test` under `lists`: the one that lists it, else its hash's.
pub fn shard_of(lists: &[Vec<String>], test: &str) -> u32 {
    lists
        .iter()
        .position(|l| l.iter().any(|t| t == test))
        .map_or_else(|| hash_shard(test, lists.len() as u32), |i| i as u32)
}

/// What one shard's runner sent back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    /// 1-based.
    pub shard: u32,
    /// How many tests the daemon listed for it.
    pub listed: u32,
    pub code: i32,
    pub signal: i32,
    pub output: String,
    pub run_s: u64,
    pub build_s: u64,
}

/// How bad an exit is: a signal beats a failure beats success.
fn severity(code: i32, signal: i32) -> (bool, bool, i32) {
    (signal != 0, code != 0, code.abs())
}

/// The shards' outputs joined in shard order and the worst exit status: `(code, signal, output)`.
/// Three outputs merged read the same to a parser as the one output of the whole run.
pub fn merge(parts: &[Part]) -> (i32, i32, String) {
    let mut sorted: Vec<&Part> = parts.iter().collect();
    sorted.sort_by_key(|p| p.shard);
    let mut out = String::new();
    for p in &sorted {
        out.push_str(&p.output);
        if !p.output.is_empty() && !p.output.ends_with('\n') {
            out.push('\n');
        }
    }
    let worst = sorted
        .iter()
        .max_by(|a, b| {
            severity(a.code, a.signal)
                .cmp(&severity(b.code, b.signal))
                // The first of equals, so the same inputs give the same status.
                .then(b.shard.cmp(&a.shard))
        })
        .map_or((0, 0), |p| (p.code, p.signal));
    (worst.0, worst.1, out)
}

/// The shards that had tests listed and ran none of them, as `(shard, listed)`. `ran` counts
/// the results in an output, with the suite's own parser.
pub fn ran_nothing(parts: &[Part], ran: &dyn Fn(&str) -> usize) -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = parts
        .iter()
        .filter(|p| p.listed > 0 && ran(&p.output) == 0)
        .map(|p| (p.shard, p.listed))
        .collect();
    v.sort_unstable();
    v
}

/// `merge`, then a red shard that ran nothing: a line saying so is added to the output and a
/// passing status becomes 1, even when the merged count passes `min_tests`.
pub fn merge_judged(parts: &[Part], ran: &dyn Fn(&str) -> usize) -> (i32, i32, String) {
    let (mut code, signal, mut output) = merge(parts);
    for (shard, listed) in ran_nothing(parts, ran) {
        if !output.is_empty() && !output.ends_with('\n') {
            output.push('\n');
        }
        output.push_str(&format!("error: shard {shard} ran 0 of {listed} tests\n"));
        if code == 0 && signal == 0 {
            code = 1;
        }
    }
    (code, signal, output)
}

/// The numbers of the shards whose artifact is not in `have`, out of `1..=shards`.
pub fn missing(shards: u32, have: &[u32]) -> Vec<u32> {
    (1..=shards).filter(|s| !have.contains(s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(n: usize) -> BTreeSet<String> {
        (0..n).map(|i| format!("t{i:03}")).collect()
    }

    fn weights(n: usize) -> BTreeMap<String, u64> {
        (0..n)
            .map(|i| (format!("t{i:03}"), 10 + (i as u64 * 37) % 400))
            .collect()
    }

    #[test]
    fn weights_give_balanced_shards() {
        let (names, w) = (set(200), weights(200));
        let lists = assign(&names, &w, 8);
        let loads: Vec<u64> = lists.iter().map(|l| l.iter().map(|t| w[t]).sum()).collect();
        let (lo, hi) = (loads.iter().min().unwrap(), loads.iter().max().unwrap());
        // Longest-first onto the lightest shard is within the heaviest single test of even.
        assert!(hi - lo <= *w.values().max().unwrap(), "{loads:?}");
    }

    #[test]
    fn the_same_inputs_give_the_same_assignment() {
        let (names, w) = (set(120), weights(120));
        assert_eq!(assign(&names, &w, 5), assign(&names, &w, 5));
    }

    #[test]
    fn every_test_is_in_exactly_one_shard() {
        let names = set(97);
        let lists = assign(&names, &weights(60), 7);
        let mut all: Vec<String> = lists.concat();
        all.sort();
        assert_eq!(all, names.iter().cloned().collect::<Vec<_>>());
    }

    #[test]
    fn an_unknown_test_takes_the_median() {
        let names: BTreeSet<String> = ["a", "b", "c"].map(String::from).into();
        let w: BTreeMap<String, u64> = [("a".to_string(), 100), ("b".to_string(), 100)].into();
        // `c` weighs 100 too: three equal tests over two shards is 2 + 1.
        let lists = assign(&names, &w, 2);
        assert_eq!(lists.iter().map(Vec::len).max(), Some(2));
    }

    #[test]
    fn a_new_test_lands_on_its_hash_shard() {
        let lists = assign(&set(30), &weights(30), 4);
        assert_eq!(
            shard_of(&lists, "brand::new_test"),
            hash_shard("brand::new_test", 4)
        );
        for t in lists.concat() {
            assert!(lists[shard_of(&lists, &t) as usize].contains(&t));
        }
    }

    #[test]
    fn the_hash_is_fnv1a() {
        // Known FNV-1a 64 values: "" is the offset basis, "a" is af63dc4c8601ec8c.
        assert_eq!(
            hash_shard("", 1 << 20),
            (0xcbf2_9ce4_8422_2325u64 % (1 << 20)) as u32
        );
        assert_eq!(
            hash_shard("a", 1 << 20),
            (0xaf63_dc4c_8601_ec8cu64 % (1 << 20)) as u32
        );
        assert_eq!(hash_shard("anything", 1), 0);
    }

    fn part(shard: u32, listed: u32, code: i32, out: &str) -> Part {
        Part {
            shard,
            listed,
            code,
            signal: 0,
            output: out.to_string(),
            run_s: 0,
            build_s: 0,
        }
    }

    fn count(o: &str) -> usize {
        o.lines().filter(|l| l.starts_with("test ")).count()
    }

    #[test]
    fn three_outputs_merged_read_as_one_combined_output() {
        let parts = [
            part(3, 1, 0, "test c ... ok\n"),
            part(1, 2, 0, "test a ... ok\ntest b ... ok\n"),
            part(2, 1, 0, "test d ... ok"),
        ];
        let (code, signal, out) = merge_judged(&parts, &count);
        assert_eq!((code, signal), (0, 0));
        assert_eq!(
            out,
            "test a ... ok\ntest b ... ok\ntest d ... ok\ntest c ... ok\n"
        );
        assert_eq!(count(&out), 4);
    }

    #[test]
    fn one_red_shard_is_red_with_its_failing_names() {
        let parts = [
            part(1, 1, 0, "test a ... ok\n"),
            part(2, 2, 101, "test b ... FAILED\ntest c ... ok\n"),
            part(3, 1, 0, "test d ... ok\n"),
        ];
        let (code, _, out) = merge_judged(&parts, &count);
        assert_eq!(code, 101);
        assert!(out.contains("test b ... FAILED"));
    }

    #[test]
    fn a_signal_is_worse_than_a_failure_and_the_first_of_equals_wins() {
        let mut killed = part(2, 1, 137, "x\n");
        killed.signal = 9;
        let (code, signal, _) = merge(&[part(1, 1, 101, "y\n"), killed]);
        assert_eq!((code, signal), (137, 9));
        let (code, _, _) = merge(&[part(2, 1, 2, ""), part(1, 1, 3, "")]);
        assert_eq!(code, 3);
    }

    #[test]
    fn a_shard_that_ran_nothing_of_a_non_empty_list_is_red() {
        let parts = [
            part(1, 1, 0, "test a ... ok\n"),
            part(2, 140, 0, "nothing here\n"),
            part(3, 0, 0, ""),
        ];
        let (code, _, out) = merge_judged(&parts, &count);
        assert_eq!(code, 1);
        assert!(out.contains("shard 2 ran 0 of 140 tests"), "{out}");
        // An empty list that ran nothing is fine.
        assert_eq!(ran_nothing(&parts, &count), vec![(2, 140)]);
    }

    #[test]
    fn missing_shards_are_the_ones_without_an_artifact() {
        assert_eq!(missing(4, &[1, 3]), vec![2, 4]);
        assert!(missing(3, &[3, 1, 2]).is_empty());
    }
}
