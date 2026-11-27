// SPDX-License-Identifier: GPL-3.0-or-later

//! A sharded suite on a node (#1252, decision 1233). A node used to take a suite's whole command
//! as one job, so an `exit-scripts` check that GitHub splits four ways ran its scripts one after
//! another there and ran out of time. Here the suite's shards are separate jobs of the same node,
//! as many at once as the node has room for, and their results are merged as the GitHub path
//! merges them ([`crate::shards::merge`]).
//!
//! Every shard runs exactly once: the jobs are taken from one list by a few workers, so fewer
//! free slots than shards only means some shards start later.

use crate::daemon::Daemon;
use crate::remote::{Fail, Finished, Outcome, RunSpec, StopWhy};
use crate::shards::Part;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};

/// How many jobs of one check run on a node at once, at most: half the node's job slots (so one
/// check cannot take every slot), never more than the shards it has, and at least one. `free` is
/// what the node says is free now: a check that finds fewer slots free than that starts with what
/// there is and takes the others as its own jobs end.
pub fn width(shards: u32, max_jobs: u64, running: u64) -> usize {
    let free = max_jobs.saturating_sub(running);
    let half = (max_jobs / 2).max(1);
    (shards as u64).min(half).min(free.max(1)).max(1) as usize
}

/// `rs` as one run per shard (1-based): each has its own id, runs `shard_command` with
/// `MASON_SHARD` and `MASON_SHARDS` set, and makes only its shard in `.mason/run.json`.
pub fn split(
    rs: &RunSpec,
    shards: u32,
    shard_command: &str,
    lists: Vec<Vec<String>>,
    partial_lists: bool,
) -> Vec<RunSpec> {
    (1..=shards)
        .map(|i| {
            let mut s = rs.clone();
            s.id = format!("{}-j{i}", rs.id);
            s.command = shard_command.to_string();
            s.shards = shards;
            s.only = vec![i];
            s.shard_command = Some(shard_command.to_string());
            s.shard_lists = lists.clone();
            s.partial_lists = partial_lists;
            s.shards_wanted = shards;
            s.env
                .retain(|(k, _)| k != "MASON_SHARD" && k != "MASON_SHARDS");
            s.env.push(("MASON_SHARD".into(), i.to_string()));
            s.env.push(("MASON_SHARDS".into(), shards.to_string()));
            s
        })
        .collect()
}

/// The jobs' results as the one result of the run: outputs in shard order, the worst status,
/// the longest time (they ran side by side), and where and how it was split.
pub fn merge(done: Vec<(usize, Finished)>, width: usize) -> Finished {
    let n = done.len();
    let parts: Vec<Part> = done
        .iter()
        .map(|(i, f)| Part {
            shard: *i as u32 + 1,
            listed: 0,
            code: f.code,
            signal: f.signal,
            output: f.output.clone(),
            run_s: f.ran_ms.unwrap_or(0) / 1000,
            build_s: 0,
        })
        .collect();
    let (mut code, signal, output) = crate::shards::merge(&parts);
    let timed_out = done.iter().any(|(_, f)| f.timed_out);
    if timed_out && code == 0 && signal == 0 {
        code = 1;
    }
    let first = done
        .first()
        .map(|(_, f)| f.ran_on.clone())
        .unwrap_or_default();
    Finished {
        code,
        signal,
        output,
        timed_out,
        stop: None,
        ran_ms: done.iter().filter_map(|(_, f)| f.ran_ms).max(),
        phase: None,
        ran_on: format!("{first}; split {n} ways, {width} at a time"),
        parts,
        missing: vec![],
    }
}

/// Polls a job that found no room waits before asking the node again, at most this many times.
fn room_tries() -> u32 {
    std::env::var("MASON_NODE_SHARD_ROOM_TRIES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(900)
}

/// Run `jobs` on node `nid`, `width` at a time, and give the merged result. A job that finds the
/// node full waits for room while one of this run's own jobs is out (and a node with no room for
/// the first is [`Fail::Environmental`], as for an unsplit run, so the caller takes the next
/// place). Any job that cannot be had, or a stop, cancels the others.
#[allow(clippy::too_many_arguments)]
pub fn execute_split(
    d: &Arc<Daemon>,
    nid: &str,
    jobs: &[RunSpec],
    width: usize,
    cwd: &Path,
    base_ref: &str,
    dir: &Path,
    stop: &(dyn Fn() -> Option<StopWhy> + Sync),
    live: &mut dyn FnMut(u64, &[u8]),
) -> Result<Outcome, Fail> {
    let next = AtomicUsize::new(0);
    let out_now = AtomicUsize::new(0);
    let abort = AtomicBool::new(false);
    let done: Mutex<Vec<(usize, Finished)>> = Mutex::new(vec![]);
    let failed: Mutex<Option<Fail>> = Mutex::new(None);
    let stopped: Mutex<Option<StopWhy>> = Mutex::new(None);
    let (tx, rx) = mpsc::channel::<(usize, Vec<u8>)>();
    let stop_all = || {
        if abort.load(Ordering::SeqCst) {
            Some(StopWhy::Skipped)
        } else {
            stop()
        }
    };
    let tries = room_tries();
    let pause = crate::node_exec::poll_interval() * 2;
    std::thread::scope(|sc| {
        for _ in 0..width.clamp(1, jobs.len().max(1)) {
            let tx = tx.clone();
            let (next, out_now, abort, done, failed, stopped, stop_all) =
                (&next, &out_now, &abort, &done, &failed, &stopped, &stop_all);
            sc.spawn(move || {
                loop {
                    let i = next.fetch_add(1, Ordering::SeqCst);
                    if i >= jobs.len() || abort.load(Ordering::SeqCst) {
                        break;
                    }
                    let mut waited = 0u32;
                    let r = loop {
                        let started = std::cell::Cell::new(false);
                        let r = crate::node_exec::attempt_noting(
                            d,
                            Some(nid),
                            &jobs[i],
                            cwd,
                            base_ref,
                            dir,
                            stop_all,
                            &mut |_, b| {
                                let _ = tx.send((i, b.to_vec()));
                            },
                            &|| {
                                started.set(true);
                                out_now.fetch_add(1, Ordering::SeqCst);
                            },
                        );
                        if started.get() {
                            out_now.fetch_sub(1, Ordering::SeqCst);
                        }
                        // Room: another job of this run is out, so one will end and free a slot.
                        let room = matches!(&r, Err(Fail::Environmental(w)) if w.starts_with("no node had room"));
                        if room && out_now.load(Ordering::SeqCst) > 0 && waited < tries {
                            waited += 1;
                            if let Some(w) = stop_all() {
                                break Ok(Outcome::Stopped(w));
                            }
                            std::thread::sleep(pause);
                            continue;
                        }
                        break r;
                    };
                    match r {
                        Ok(Outcome::Done(f)) => done
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push((i, f)),
                        Ok(Outcome::Stopped(w)) => {
                            if !abort.swap(true, Ordering::SeqCst) {
                                *stopped.lock().unwrap_or_else(|e| e.into_inner()) = Some(w);
                            }
                        }
                        Err(e) => {
                            if !abort.swap(true, Ordering::SeqCst) {
                                *failed.lock().unwrap_or_else(|e| e.into_inner()) = Some(e);
                            }
                        }
                    }
                }
            });
        }
        drop(tx);
        // Progress is told whole lines, one job's at a time.
        let mut bufs: Vec<Vec<u8>> = vec![vec![]; jobs.len()];
        let mut off = 0u64;
        while let Ok((i, b)) = rx.recv() {
            bufs[i].extend_from_slice(&b);
            if let Some(end) = bufs[i].iter().rposition(|c| *c == b'\n') {
                let whole: Vec<u8> = bufs[i].drain(..=end).collect();
                live(off, &whole);
                off += whole.len() as u64;
            }
        }
        for b in bufs.iter().filter(|b| !b.is_empty()) {
            live(off, b);
            off += b.len() as u64;
        }
    });
    if let Some(w) = stopped.into_inner().unwrap_or_else(|e| e.into_inner()) {
        return Ok(Outcome::Stopped(w));
    }
    if let Some(e) = failed.into_inner().unwrap_or_else(|e| e.into_inner()) {
        return Err(e);
    }
    let mut done = done.into_inner().unwrap_or_else(|e| e.into_inner());
    done.sort_by_key(|(i, _)| *i);
    Ok(Outcome::Done(merge(done, width)))
}

// ---- passes kept by the coordinator (#1252) ------------------------------------------------

/// The directory under the task's cache where passes of runs off this Mac are kept.
const KEPT: &str = "exit-scripts-pass-remote";

/// The lines a runner prints for each pass, which the coordinator reads and removes.
const KEY_LINE: &str = "MASON-EXIT-KEY ";

fn is_key(k: &str) -> bool {
    k.len() == 64 && k.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The environment of a run's `exit-scripts` job that lets a runner skip what already passed:
/// only for an agent's check (`MASON_EXIT_CACHE=1`) with a task cache directory, in a suite named
/// `exit-scripts`. `tree` is the key of the tree it is sent. Empty when nothing is kept.
pub fn kept_env(env: &[(String, String)], suite: &str, tree: &str) -> Vec<(String, String)> {
    let get = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
    if get("MASON_EXIT_CACHE") != Some("1") || suite != "exit-scripts" || tree.is_empty() {
        return vec![];
    }
    let Some(cache) = get("MASON_TASK_CACHE").filter(|c| Path::new(c).is_dir()) else {
        return vec![];
    };
    let mut keys: Vec<String> = std::fs::read_dir(Path::new(cache).join(KEPT))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|k| is_key(k))
        .collect();
    keys.sort();
    vec![
        ("MASON_EXIT_TREE_KEY".into(), tree.to_string()),
        ("MASON_EXIT_PASSES".into(), keys.join(" ")),
    ]
}

/// Keep the passes a run reported (its `MASON-EXIT-KEY` lines) under the task's cache, and give
/// the output back without those lines. Output of a run that sent no `MASON_EXIT_TREE_KEY` is
/// returned as it is.
pub fn keep_passes(env: &[(String, String)], sent: &[(String, String)], output: &str) -> String {
    if !sent.iter().any(|(k, _)| k == "MASON_EXIT_TREE_KEY") {
        return output.to_string();
    }
    let cache = env
        .iter()
        .find(|(k, _)| k == "MASON_TASK_CACHE")
        .map(|(_, v)| Path::new(v).join(KEPT));
    let mut out = String::with_capacity(output.len());
    for line in output.split_inclusive('\n') {
        match line.trim_end().strip_prefix(KEY_LINE) {
            Some(k) => {
                if let (Some(dir), true) = (cache.as_ref(), is_key(k.trim())) {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(dir.join(k.trim()), b"");
                }
            }
            None => out.push_str(line),
        }
    }
    out
}

/// A line for a result in which exit scripts were `not run: out of time`: how many ran, how long
/// the run took, how many ways it was split, and whether a rerun goes on from kept passes, so an
/// agent does not rerun blindly. `None` when every selected script was run.
pub fn out_of_time_note(
    output: &str,
    secs: u64,
    ways: u32,
    wanted: u32,
    kept: bool,
) -> Option<String> {
    let (mut ran, mut cached, mut not_run) = (0u32, 0u32, 0u32);
    for l in output.lines() {
        if l.starts_with("FAIL ") && l.contains("(not run: out of time") {
            not_run += 1;
        } else if l.starts_with("PASS ") && l.ends_with("(cached)") {
            cached += 1;
        } else if (l.starts_with("PASS ") && !l.starts_with("PASS none")) || l.starts_with("FAIL ")
        {
            ran += 1;
        }
    }
    if not_run == 0 {
        return None;
    }
    let split = match (ways, wanted) {
        (w, n) if n > 1 && w < n => {
            format!("split {w} ways, fewer than the {n} the suite asks for")
        }
        (w, _) if w > 1 => format!("split {w} ways"),
        _ => "not split".to_string(),
    };
    let advice = match (kept, ran) {
        (true, 0) => "no script finished in the time given, so another run starts at the same place: do not run it again unchanged",
        (true, _) => "run the check again: the scripts that passed are kept and it goes on with the rest",
        (false, _) => "the passes of this run are not kept, so another run starts again from the first script",
    };
    Some(format!(
        "exit-scripts ran out of time: {ran} ran and {cached} were already passed, {not_run} not run, in {} min, {split}; {advice}",
        secs.div_ceil(60)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rs() -> RunSpec {
        crate::remote::spec_for(
            "p",
            "t1-check",
            None,
            "bash all.sh",
            std::time::Duration::from_secs(60),
            &[],
            "node",
            "tree",
        )
    }

    #[test]
    fn a_check_takes_at_most_half_the_slots_and_at_least_one() {
        assert_eq!(width(4, 12, 0), 4);
        assert_eq!(width(8, 12, 0), 6);
        assert_eq!(width(8, 12, 10), 2);
        assert_eq!(width(4, 12, 12), 1);
        assert_eq!(width(4, 1, 0), 1);
        assert_eq!(width(1, 12, 0), 1);
    }

    #[test]
    fn each_shard_is_one_job_with_its_own_number() {
        let jobs = split(&rs(), 3, "bash shard.sh", vec![], false);
        assert_eq!(jobs.len(), 3);
        for (i, j) in jobs.iter().enumerate() {
            let n = (i + 1).to_string();
            assert_eq!(j.command, "bash shard.sh");
            assert_eq!(j.only, vec![i as u32 + 1]);
            assert!(j.env.contains(&("MASON_SHARD".into(), n)));
            assert!(j.env.contains(&("MASON_SHARDS".into(), "3".into())));
        }
        let ids: std::collections::BTreeSet<_> = jobs.iter().map(|j| j.id.clone()).collect();
        assert_eq!(ids.len(), 3);
    }

    fn fin(code: i32, out: &str) -> Finished {
        Finished {
            code,
            signal: 0,
            output: out.into(),
            timed_out: false,
            stop: None,
            ran_ms: Some(2000),
            phase: None,
            ran_on: "ran on node b".into(),
            parts: vec![],
            missing: vec![],
        }
    }

    #[test]
    fn one_red_job_makes_the_merge_red_and_keeps_every_script() {
        let m = merge(
            vec![
                (1, fin(1, "FAIL s2\n")),
                (0, fin(0, "PASS s1\n")),
                (2, fin(0, "PASS s3")),
            ],
            2,
        );
        assert_eq!(m.code, 1);
        assert_eq!(m.output, "PASS s1\nFAIL s2\nPASS s3\n");
        assert!(m.ran_on.contains("split 3 ways, 2 at a time"));
    }

    #[test]
    fn passes_are_kept_from_the_output_and_sent_back_with_the_next_run() {
        let cache = tempfile::tempdir().unwrap();
        let key = "a".repeat(64);
        let env = vec![
            ("MASON_EXIT_CACHE".to_string(), "1".to_string()),
            (
                "MASON_TASK_CACHE".to_string(),
                cache.path().to_string_lossy().into_owned(),
            ),
        ];
        let first = kept_env(&env, "exit-scripts", "tree1");
        assert_eq!(first.len(), 2);
        assert_eq!(first[1].1, "");
        let text = format!("PASS s1\n{KEY_LINE}{key}\nMASON-EXIT-KEY ../escape\nFAIL s2\n");
        let shown = keep_passes(&env, &first, &text);
        assert_eq!(shown, "PASS s1\nFAIL s2\n");
        let second = kept_env(&env, "exit-scripts", "tree1");
        assert_eq!(second[1].1, key);
        // A test step or a landing run has no MASON_EXIT_CACHE: nothing is kept or sent.
        assert!(kept_env(&env[1..], "exit-scripts", "tree1").is_empty());
        assert!(kept_env(&env, "exit-scripts-landing", "tree1").is_empty());
        assert_eq!(keep_passes(&env[1..], &[], &text), text);
    }

    #[test]
    fn a_run_out_of_time_says_what_ran_and_whether_a_rerun_helps() {
        let out = "PASS s1\nPASS s2 (cached)\nFAIL s3 (not run: out of time; run the check again to continue)\n";
        let n = out_of_time_note(out, 5400, 4, 4, true).unwrap();
        assert!(
            n.contains("1 ran and 1 were already passed, 1 not run, in 90 min, split 4 ways"),
            "{n}"
        );
        assert!(n.contains("kept"));
        let none = "FAIL s3 (not run: out of time; run the check again to continue)\n";
        assert!(out_of_time_note(none, 60, 1, 4, true)
            .unwrap()
            .contains("do not run it again"));
        assert!(out_of_time_note("PASS s1\n", 60, 1, 4, true).is_none());
    }
}
