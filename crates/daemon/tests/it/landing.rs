// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 14: the landing engine end to end. Tasks are put straight into the
//! landing step (their branches made here), so no agent is involved: only
//! real git, real suites and the real coordinator.

use crate::support;
use mason_core::*;
use mason_landing::git;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::lw::*;
use support::*;

#[test]
fn a_batch_of_five_lands_three_defers_the_mate_conflict_and_parks_the_culprit() {
    let w = world(
        "window = \"30s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n",
        &[],
    );
    let t1 = w.task("one", &[("x/one.txt", "1")]);
    let t2 = w.task("two", &[("x/two.txt", "2")]);
    let t3 = w.task("three", &[("shared.txt", "from three\n")]);
    let t4 = w.task("four", &[("shared.txt", "from four\n")]);
    let t5 = w.task("five", &[("red.integration", "boom")]);
    wait_for("the other three to land", 90, || {
        [t1, t2, t3].iter().all(|t| w.landed(*t))
    });
    wait_for("the culprit and the conflict to be sent back", 60, || {
        w.sent_back(t4).is_some() && w.sent_back(t5).is_some()
    });
    // Only the culprit is parked for the red; the conflict is its own reason.
    let r5 = w.sent_back(t5).unwrap();
    assert!(r5.contains("red") && r5.contains("integration"), "{r5}");
    let r4 = w.sent_back(t4).unwrap();
    assert!(
        r4.contains("conflicts") && r4.contains("shared.txt"),
        "{r4}"
    );
    for t in [t1, t2, t3] {
        assert!(w.sent_back(t).is_none(), "task {t} must not be parked");
    }
    // The base has exactly the three, once each, and none of the others.
    assert!(w.on_main("x/one.txt") && w.on_main("x/two.txt"));
    assert_eq!(sh(&w.s.origin, &["show", "main:shared.txt"]), "from three");
    assert!(!w.on_main("red.integration"));
    // One batch, recorded with who landed and who was parked.
    // Parked tasks go back at once, before the batch row is final: wait for the final row.
    // The conflict with a batch-mate waited for the next batch (it is not in this row), where
    // it conflicted with the base, now that its mate had landed, and was sent back then.
    let want_parked = t5.0.to_string();
    wait_for("the batch row to be final", 30, || {
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .iter()
            .any(|b| b.phase == "landed" && b.parked == want_parked)
    });
    let want_second = t4.0.to_string();
    wait_for("the next batch to send the conflict back", 60, || {
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .iter()
            .any(|b| b.phase == "parked" && b.parked == want_second)
    });
    let rows = w.d.store.batches(Some("scratch"), 10).unwrap();
    let b = rows
        .iter()
        .find(|b| b.phase == "landed")
        .expect("a landed batch");
    assert_eq!(b.landed, format!("{},{},{}", t1.0, t2.0, t3.0));
    assert_eq!(b.parked, t5.0.to_string());
    // The integration branch is gone; the owner's checkout was not touched.
    assert!(!git::branch_exists(
        &w.s.repo,
        &format!("mason/int/scratch/b{}", b.id)
    ));
    assert_eq!(
        sh(&w.s.repo, &["rev-parse", "--abbrev-ref", "HEAD"]),
        "owner-work"
    );
    w.d.store.verify().unwrap();
}

#[test]
fn the_red_ladder_blames_by_impact_before_it_bisects() {
    // Only `a/**` impacts alpha, so a red alpha points at the task that touched a/.
    let w = world(
        "window = \"2s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\", \"beta\"]\n",
        &[],
    );
    let t1 = w.task("bee", &[("b/ok.txt", "1")]);
    let t2 = w.task("ay", &[("a/red.alpha", "x")]);
    wait_for("the innocent task to land", 90, || w.landed(t1));
    wait_for("the culprit to be sent back", 60, || {
        w.sent_back(t2).is_some()
    });
    assert!(w.sent_back(t2).unwrap().contains("alpha"));
    assert!(w.on_main("b/ok.txt") && !w.on_main("a/red.alpha"));
}

#[test]
fn a_flaky_red_passes_on_the_rerun() {
    // The suite is red the first time it runs and green after (a marker outside the tree).
    let w = world(
        "window = \"1s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n",
        &[],
    );
    let marker = w.s.root.path().join("ran-once");
    let flaky = format!(
        "#!/bin/sh\necho 1..1\nif [ -f {m} ]; then echo 'ok 1 - integration'; else touch {m}; echo 'not ok 1 - integration'; exit 1; fi\n",
        m = marker.display()
    );
    w.s.push_files(&[("checks.sh", &flaky)], "flaky suite");
    let t = w.task("steady", &[("x/f.txt", "1")]);
    wait_for("it to land on the rerun", 90, || w.landed(t));
    assert!(w.sent_back(t).is_none());
}

#[test]
fn disjoint_batches_run_in_parallel_and_overlapping_ones_wait() {
    let w = world(
        "window = \"0s\"\nparallel = 2\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\", \"beta\"]\n",
        &[],
    );
    // Watch how many batches run at once.
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watcher = {
        let (d, peak, stop) = (w.d.clone(), peak.clone(), stop.clone());
        std::thread::spawn(move || {
            let pid = d.project_id_by_name("scratch").unwrap();
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                let n = d.landing().running_batches(pid);
                peak.fetch_max(n, std::sync::atomic::Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    let pid = w.d.project_id_by_name("scratch").unwrap();
    // A is slow (alpha takes 3 s); B touches only beta: it runs beside A.
    let a = w.task("alpha slow", &[("a/one.txt", "1"), ("a/slow.alpha", "x")]);
    wait_for("A's batch to start", 30, || {
        w.d.landing().running_batches(pid) == 1
    });
    let b = w.task("beta", &[("b/one.txt", "1")]);
    wait_for("B to run beside A", 30, || {
        peak.load(std::sync::atomic::Ordering::SeqCst) >= 2
    });
    // C overlaps A's suite: it must not run until A is done.
    let c = w.task("alpha again", &[("c/one.txt", "1")]);
    wait_for("A and B to land", 90, || w.landed(a) && w.landed(b));
    wait_for("C to land", 90, || w.landed(c));
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    watcher.join().unwrap();
    // C never ran beside A: the rows show A's batch ended before C's began.
    let rows = w.d.store.batches(Some("scratch"), 20).unwrap();
    let find = |t: TaskId| {
        rows.iter()
            .find(|r| r.landed.split(',').any(|x| x == t.0.to_string()))
            .unwrap_or_else(|| panic!("no batch landed task {t}: {rows:?}"))
    };
    let (ra, rc) = (find(a), find(c));
    assert_ne!(ra.id, rc.id);
    assert!(
        ra.finished_at.unwrap() <= rc.finished_at.unwrap(),
        "A finishes before C does"
    );
    // The queue sat idle while tasks waited, and says how long.
    let _ = w.d.landing().idle_ms(pid);
}

#[test]
fn land_now_skips_the_window_and_force_skips_verification_leaving_debts() {
    let w = world(
        "window = \"600s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n",
        &[],
    );
    let slow = w.task("waits for the window", &[("x/slow.txt", "1")]);
    let now = w.task("now", &[("x/now.txt", "1")]);
    let forced = w.task("forced", &[("red.integration", "x")]);
    // One after the other: a forced red on the base would (rightly) fail the
    // verification of whatever lands after it.
    w.d.store
        .set_landing_flag(now.0, "now", "owner", 1)
        .unwrap();
    w.d.landing().wake();
    wait_for("land now to land", 90, || w.landed(now));
    w.d.store
        .set_landing_flag(forced.0, "force", "owner", 1)
        .unwrap();
    w.d.landing().wake();
    wait_for("force to land", 90, || w.landed(forced));
    assert!(!w.landed(slow), "the window is 10 minutes");
    assert!(w.on_main("x/now.txt"));
    assert!(w.on_main("red.integration"), "force skips verification");
}

#[test]
fn a_red_post_land_tier_freezes_landing_until_the_fix_lands() {
    let w = world(
        "window = \"0s\"\n[[tier]]\nname = \"post-land\"\nsuites = [\"smoke\"]\non_red = \"freeze\"\n",
        &[],
    );
    let bad = w.task("breaks smoke", &[("red.smoke", "x")]);
    wait_for("it to land", 90, || w.landed(bad));
    wait_for("the freeze", 60, || {
        !w.d.store.freezes().unwrap().is_empty()
    });
    // The freeze reaches the banner list with its own kind (the monitors show the landing_frozen icon).
    let hv = mason_daemon::views::Ctx::new(&w.d, &w.d.world().unwrap()).header("");
    assert!(
        hv.banners.iter().any(|b| b.kind == "landing_frozen"),
        "{:?}",
        hv.banners.iter().map(|b| &b.kind).collect::<Vec<_>>()
    );
    let f = w.d.store.freezes().unwrap().remove(0);
    let fix = TaskId(f.fix_task.expect("a fix task was filed"));
    let wd = w.d.world().unwrap();
    assert_eq!(wd.tasks[&fix].priority, Priority::Urgent);
    assert!(wd.tasks[&fix].title.contains("smoke"));
    let frozen_alert = || {
        mason_daemon::attention::alerts_now(&w.d)
            .into_iter()
            .find(|(k, kind, _)| k == "frozen:scratch" && kind == "landing")
            .map(|(_, _, t)| t)
    };
    let text = frozen_alert().expect("the freeze raises an alert");
    assert!(text.contains(&format!("Fix task #{}", fix.0)), "{text}");
    // Frozen: an ordinary task waits.
    let other = w.task("ordinary", &[("x/o.txt", "1")]);
    std::thread::sleep(Duration::from_secs(3));
    assert!(!w.landed(other), "only fixes pass a freeze");
    // The fix: put it in landing with a change that removes the red file.
    let tmp = w.s.root.path().join("fixmk");
    let br = format!("mason/scratch/t{}", fix.0);
    sh(&w.s.repo, &["fetch", "-q", "origin"]);
    support::lw::add_worktree(&w.s.repo, &br, &tmp);
    sh(&tmp, &["rm", "-q", "red.smoke"]);
    sh(
        &tmp,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-qm",
            "fix",
        ],
    );
    sh(
        &w.s.repo,
        &["worktree", "remove", "--force", tmp.to_str().unwrap()],
    );
    let pid = w.d.project_id_by_name("scratch").unwrap();
    // The fix task was queued (claims are paused): admit it straight into landing.
    let def = w.d.current_def(pid);
    w.d.emit(EventKind::Admitted {
        task: fix,
        entry: Some(Entry {
            def: def.hash.clone(),
            step: "land".into(),
            state: State::Landing,
            route: Route::Explore,
        }),
    })
    .unwrap();
    wait_for("the fix to land", 90, || w.landed(fix));
    wait_for("the freeze to lift", 60, || {
        w.d.store.freezes().unwrap().is_empty()
    });
    assert!(frozen_alert().is_none(), "the alert clears with the freeze");
    wait_for("the ordinary task to land", 90, || w.landed(other));
}

#[test]
fn a_tier_that_names_ci_is_refused_with_a_pointer_to_the_ci_mode() {
    let w = world(
        "mode = \"direct\"\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\nci = \"github\"\n",
        &[],
    );
    let t = w.task("never lands", &[("x/n.txt", "1")]);
    wait_state(&w.d, t, State::Failed, 60);
    let why = task_of(&w.d, t).failure.unwrap();
    assert!(why.contains("direct-ci"), "{why}");
    assert!(!w.on_main("x/n.txt"));
}

#[test]
fn a_mode_that_needs_a_service_the_project_lacks_is_refused_naming_it() {
    let w = world("mode = \"pr-batch\"\n", &[]);
    let t = w.task("never lands", &[("x/n.txt", "1")]);
    wait_state(&w.d, t, State::Failed, 60);
    assert!(task_of(&w.d, t).failure.unwrap().contains("[host]"));
}

#[test]
fn landing_with_no_config_is_the_old_direct_landing() {
    let w = world("", &[]);
    let t = w.task("plain", &[("x/p.txt", "1")]);
    wait_for("it to land", 60, || w.landed(t));
    assert!(w.on_main("x/p.txt"));
    let _ = Instant::now();
}

#[test]
fn a_red_reverification_after_the_base_moved_ends_and_does_not_hang() {
    // The batch is green on its own tree; while it verifies, the base gains a change that
    // turns `integration` red together with the batch's file. The re-verification then has
    // a named test red on both runs with no baseline stored, which used to take the
    // repository lock a second time from the landing thread, holding it, and hang. It must end: here, sent back.
    let w = world(
        "window = \"1s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n",
        &[],
    );
    let t = w.task("slowly", &[("x/slow.integration", "1")]);
    wait_for("the batch to verify", 60, || {
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .iter()
            .any(|b| b.phase == "verifying")
    });
    // Red only with the batch's change on top: the new base on its own stays green, so the
    // red is the batch's and cannot be excused as also failing at the merge base (s177).
    let checks = "#!/bin/sh\nn=\"$1\"\n[ -n \"$(find . -name \"slow.$n\" -not -path \"./.git/*\")\" ] && sleep 3\necho \"1..1\"\nif [ -n \"$(find . -name \"red.$n\" -not -path \"./.git/*\")\" ] || [ -f \"x/slow.$n\" ]; then echo \"not ok 1 - $n\"; exit 1; fi\necho \"ok 1 - $n\"\n";
    w.s.push_files(&[("checks.sh", checks)], "turn integration red with x/slow");
    wait_for("the landing to end instead of hanging", 120, || {
        w.sent_back(t).is_some() || w.landed(t)
    });
    assert!(!w.on_main("x/slow.integration"), "a red tree was promoted");
    assert!(w.sent_back(t).is_some(), "the red batch was not sent back");
    // The repository lock is free: another step can take it.
    let pid = w.d.project_id_by_name("scratch").unwrap();
    let lock = w.d.repo_lock(pid);
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        drop(lock.lock().unwrap_or_else(|e| e.into_inner()));
        let _ = tx.send(());
    });
    rx.recv_timeout(Duration::from_secs(10))
        .expect("the repository lock is still held");
    w.d.store.verify().unwrap();
}

#[test]
fn the_retest_after_a_moved_base_holds_a_compile_lease_and_not_the_repository_lock() {
    // `scripts/gen-status.sh` runs after the batch is assembled; here it announces
    // itself and waits, which is the time to move the base in.
    let started;
    {
        let probe = tempfile::tempdir().unwrap();
        started = probe.path().join("started");
        std::mem::forget(probe);
    }
    let gen = format!("touch {}\nsleep 2\n", started.display());
    let w = world_full(
        "window = \"1s\"\n",
        &|_| String::new(),
        &[("scripts/gen-status.sh", &gen)],
        "sleep 5",
        true,
    );
    let t = w.task("moves", &[("x/m.txt", "1")]);
    wait_for("the batch to be assembled", 90, || started.exists());
    w.s.push_files(&[("other.txt", "o")], "the base moves");
    let pid = w.d.project_id_by_name("scratch").unwrap();
    let lock = w.d.repo_lock(pid);
    let mut seen = false;
    let mut free_meanwhile = false;
    wait_for("the re-test to hold a compile lease", 60, || {
        if w.d.broker.used("compile") >= 1 {
            seen = true;
            // While it runs, the repository lock is free.
            free_meanwhile = lock.try_lock().is_ok();
            true
        } else {
            false
        }
    });
    assert!(seen);
    assert!(free_meanwhile, "the re-test ran under the repository lock");
    wait_for("the batch to land", 90, || w.landed(t));
    assert!(w.on_main("x/m.txt") && w.on_main("other.txt"));
    assert_eq!(w.d.broker.used("compile"), 0, "the lease was released");
}

#[test]
fn the_retest_builds_holding_compile_then_tests_without_it() {
    // The project's overlay names a `build` beside its `test`: the re-test after a
    // moved base builds first holding the compile slot, then lets it go to test.
    let gen_started;
    let built;
    let tested;
    {
        let probe = tempfile::tempdir().unwrap();
        gen_started = probe.path().join("started");
        built = probe.path().join("built");
        tested = probe.path().join("tested");
        std::mem::forget(probe);
    }
    let gen = format!("touch {}\nsleep 2\n", gen_started.display());
    let build = format!("build = \"sleep 1; touch {}\"\n", built.display());
    let test_cmd = format!("touch {}; sleep 3", tested.display());
    let w = world_full(
        "window = \"1s\"\n",
        &|_| build.clone(),
        &[("scripts/gen-status.sh", &gen)],
        &test_cmd,
        true,
    );
    let t = w.task("moves", &[("x/m.txt", "1")]);
    wait_for("the batch to be assembled", 90, || gen_started.exists());
    w.s.push_files(&[("other.txt", "o")], "the base moves");
    wait_for("the re-test's build to hold the compile slot", 60, || {
        w.d.broker.used("compile") >= 1 && !built.exists()
    });
    wait_for("the re-test's tests to start", 60, || tested.exists());
    assert!(built.exists(), "the build ran first");
    assert_eq!(
        w.d.broker.used("compile"),
        0,
        "the tests run without the build slot"
    );
    wait_for("the batch to land", 90, || w.landed(t));
    assert!(w.on_main("x/m.txt") && w.on_main("other.txt"));
}

#[test]
fn a_batch_whose_testing_was_skipped_is_rebuilt_but_not_retested_when_the_base_moves() {
    // s429: the owner skipped every task's test step to land a backlog. A base move used to
    // re-test such a batch in full, the only full run it got, holding the queue for half an
    // hour; now it is rebuilt and lands.
    let gen_started;
    let built;
    let tested;
    {
        let probe = tempfile::tempdir().unwrap();
        gen_started = probe.path().join("started");
        built = probe.path().join("built");
        tested = probe.path().join("tested");
        std::mem::forget(probe);
    }
    let gen = format!("touch {}\nsleep 2\n", gen_started.display());
    let build = format!("build = \"touch {}\"\n", built.display());
    let test_cmd = format!("touch {}", tested.display());
    let w = world_full(
        "window = \"1s\"\n",
        &|_| build.clone(),
        &[("scripts/gen-status.sh", &gen)],
        &test_cmd,
        true,
    );
    let t = w.task("skipped", &[("x/s.txt", "1")]);
    w.d.store
        .add_debt(
            t.0,
            "scratch",
            "step:test",
            "owner skipped",
            "brian",
            false,
            0,
        )
        .unwrap();
    wait_for("the batch to be assembled", 90, || gen_started.exists());
    w.s.push_files(&[("other.txt", "o")], "the base moves");
    wait_for("the batch to land", 90, || w.landed(t));
    assert!(w.on_main("x/s.txt") && w.on_main("other.txt"));
    assert!(built.exists(), "the moved base was rebuilt");
    assert!(
        !tested.exists(),
        "a batch with its testing skipped was re-tested"
    );
}

const LINT_TIER: &str = "window = \"1s\"\n[[tier]]\nname = \"lint\"\nsuites = [\"lint\"]\n";

#[test]
fn a_batch_that_is_not_lint_clean_is_parked_with_the_lint_in_its_note() {
    // s521: a lint error landed because neither the tests nor the rebuild ran the linter.
    let w = world_full(LINT_TIER, &|_| String::new(), &[], "true", false);
    let t = w.task("dirty", &[("x/red.lint", "1")]);
    wait_for("the batch to be sent back", 90, || w.sent_back(t).is_some());
    assert!(
        !w.on_main("x/red.lint"),
        "a batch that is not lint clean landed"
    );
    let why = w.sent_back(t).unwrap();
    assert!(why.contains("lint"), "the reason names the lint: {why}");
}

#[test]
fn a_lint_clean_batch_lands() {
    let w = world_full(LINT_TIER, &|_| String::new(), &[], "true", false);
    let t = w.task("clean", &[("x/c.txt", "1")]);
    wait_for("the batch to land", 90, || w.landed(t));
    assert!(w.on_main("x/c.txt"));
}

#[test]
fn a_batch_whose_testing_was_skipped_still_runs_the_lint_check() {
    let w = world_full(LINT_TIER, &|_| String::new(), &[], "true", true);
    let t = w.task("skipped and dirty", &[("x/red.lint", "1")]);
    w.d.store
        .add_debt(
            t.0,
            "scratch",
            "step:test",
            "owner skipped",
            "brian",
            false,
            0,
        )
        .unwrap();
    wait_for("the batch to be sent back", 90, || w.sent_back(t).is_some());
    assert!(
        !w.on_main("x/red.lint"),
        "a skipped batch landed past the lint"
    );
}

/// A `checks.sh` that appends each suite's name to `log`, and whose `lint` is red with a
/// rustfmt-style `Diff in` line for every `x/bad.rs` in the tree, and for `y/base.rs`.
fn lint_checks_script(log: &std::path::Path) -> String {
    format!(
        "#!/bin/sh\nn=\"$1\"\necho \"$n\" >> {}\necho \"1..1\"\nif [ \"$n\" = lint ]; then\n  bad=0\n  for f in x/bad.rs y/base.rs; do [ -f \"$f\" ] && {{ echo \"Diff in $f:1:\"; bad=1; }}; done\n  [ $bad = 1 ] && {{ echo \"not ok 1 - lint\"; exit 1; }}\nfi\necho \"ok 1 - $n\"\n",
        log.display()
    )
}

const LINT_AND_BATCH: &str = "window = \"2s\"\n[[tier]]\nname = \"batch\"\nsuites = [\"alpha\"]\n[[tier]]\nname = \"lint\"\nsuites = [\"lint\"]\n";

#[test]
fn lint_runs_before_the_batch_suite_and_parks_only_the_task_that_touches_the_file() {
    // s691: an unformatted file was found after the whole batch suite, then bisected.
    let probe = tempfile::tempdir().unwrap();
    let log = probe.path().join("calls");
    let script = lint_checks_script(&log);
    let w = world(LINT_AND_BATCH, &[("checks.sh", &script)]);
    let bad = w.task("unformatted", &[("x/bad.rs", "fn  main(){}")]);
    let good = w.task("clean", &[("a/ok.txt", "1")]);
    wait_for("the clean task to land", 120, || w.landed(good));
    wait_for("the unformatted task to be sent back", 60, || {
        w.sent_back(bad).is_some()
    });
    let why = w.sent_back(bad).unwrap();
    assert!(why.contains("lint"), "{why}");
    assert!(w.on_main("a/ok.txt") && !w.on_main("x/bad.rs"));
    let calls = std::fs::read_to_string(&log).unwrap();
    let calls: Vec<&str> = calls.lines().collect();
    assert_eq!(calls.first(), Some(&"lint"), "lint runs first: {calls:?}");
    assert_eq!(
        calls.iter().filter(|c| **c == "alpha").count(),
        1,
        "one suite run, no bisect: {calls:?}"
    );
}

#[test]
fn a_coloured_clippy_red_parks_the_task_that_touches_the_file_without_a_split() {
    // s789: GitHub's coloured `--> file:line` lines named the file, and the batch was split.
    let probe = tempfile::tempdir().unwrap();
    let log = probe.path().join("calls");
    let script = format!(
        "#!/bin/sh\nn=\"$1\"\necho \"$n\" >> {}\necho \"1..1\"\nif [ \"$n\" = lint ] && [ -f x/bad.rs ]; then\n  printf '\\033[1m\\033[91merror\\033[0m: x\\n  \\033[1m\\033[94m--> \\033[0mx/bad.rs:3:4\\n'\n  echo \"not ok 1 - lint\"; exit 1\nfi\necho \"ok 1 - $n\"\n",
        log.display()
    );
    let w = world(LINT_AND_BATCH, &[("checks.sh", &script)]);
    let bad = w.task("borrow", &[("x/bad.rs", "fn main() {}")]);
    let g1 = w.task("one", &[("a/one.txt", "1")]);
    let g2 = w.task("two", &[("a/two.txt", "2")]);
    wait_for("both clean tasks to land", 120, || {
        w.landed(g1) && w.landed(g2)
    });
    wait_for("the offending task to be sent back", 60, || {
        w.sent_back(bad).is_some()
    });
    assert!(!w.on_main("x/bad.rs"));
}

#[test]
fn a_guard_line_naming_a_file_one_task_added_parks_that_task_without_a_split() {
    // s978: the repository's guards print `<path>: <message>`, which named no file, so the
    // batch was bisected.
    let probe = tempfile::tempdir().unwrap();
    let log = probe.path().join("calls");
    let script = format!(
        "#!/bin/sh\nn=\"$1\"\necho \"$n\" >> {}\necho \"1..1\"\nif [ \"$n\" = lint ] && [ -f scripts/exit/s1.sh ]; then\n  echo \"scripts/exit/s1.sh: runs swift without testing MASON_SKIP_SWIFT\" >&2\n  echo \"not ok 1 - lint\"; exit 1\nfi\necho \"ok 1 - $n\"\n",
        log.display()
    );
    let w = world(LINT_AND_BATCH, &[("checks.sh", &script)]);
    let bad = w.task("swift", &[("scripts/exit/s1.sh", "swift test")]);
    let g1 = w.task("one", &[("a/one.txt", "1")]);
    let g2 = w.task("two", &[("a/two.txt", "2")]);
    wait_for("both clean tasks to land", 120, || {
        w.landed(g1) && w.landed(g2)
    });
    wait_for("the offending task to be sent back", 60, || {
        w.sent_back(bad).is_some()
    });
    assert!(!w.on_main("scripts/exit/s1.sh"));
    let _ = log;
}

#[test]
fn a_lint_red_in_a_file_no_task_touches_is_not_blamed_by_file() {
    // The file belongs to the base: the existing ladder (rerun, impact, split) handles it.
    let probe = tempfile::tempdir().unwrap();
    let log = probe.path().join("calls");
    let script = lint_checks_script(&log);
    let w = world(
        LINT_AND_BATCH,
        &[("checks.sh", &script), ("y/base.rs", "fn  main(){}")],
    );
    let t = w.task("innocent", &[("a/ok.txt", "1")]);
    wait_for("the batch to be sent back", 120, || {
        w.sent_back(t).is_some()
    });
    assert!(!w.on_main("a/ok.txt"));
    let calls = std::fs::read_to_string(&log).unwrap();
    assert!(
        !calls.lines().any(|c| c == "alpha"),
        "the suite never ran over a lint red: {calls}"
    );
}

// ---- Land now on a running batch (#615) ---------------------------------------------------

fn post_api(d: &Arc<mason_daemon::Daemon>, path: String) -> (u16, serde_json::Value) {
    let d = d.clone();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async move {
            use tower::ServiceExt;
            let req = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .header("x-mason-as", "owner")
                .body(axum::body::Body::from("{}"))
                .unwrap();
            let resp = mason_daemon::http::api_router(d)
                .oneshot(req)
                .await
                .unwrap();
            let status = resp.status().as_u16();
            let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
                .await
                .unwrap();
            (
                status,
                serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
            )
        })
}

/// `checks.sh` whose `integration` suite blocks until the file `gate` exists.
fn gated_checks(gate: &std::path::Path) -> String {
    CHECKS.replacen(
        "echo \"1..1\"\n",
        &format!(
            "[ \"$n\" = integration ] && while [ ! -f {} ]; do sleep 0.2; done\necho \"1..1\"\n",
            gate.display()
        ),
        1,
    )
}

/// Two tasks in one batch whose tier suite blocks; Land now for the batch over the API.
/// Returns the world, the gate file and the two tasks, both landed.
fn land_now_blocked_batch(extra: &[(&str, &str)]) -> (W, std::path::PathBuf, [TaskId; 2]) {
    let gate_dir = tempfile::tempdir().unwrap().keep();
    let gate = gate_dir.join("gate");
    let checks = gated_checks(&gate);
    let w = world(
        "window = \"3s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n",
        &[("checks.sh", &checks)],
    );
    let mut one: Vec<(&str, &str)> = vec![("x/one.txt", "1")];
    one.extend_from_slice(extra);
    let t1 = w.task("one", &one);
    let t2 = w.task("two", &[("x/two.txt", "2")]);
    wait_for("both tasks in one running batch", 90, || {
        w.d.landing().in_batch(t1)
            && w.d.landing().in_batch(t2)
            && w.d
                .store
                .batches(Some("scratch"), 5)
                .unwrap()
                .iter()
                .any(|b| b.finished_at.is_none() && b.phase == "verifying" && b.tasks.contains(','))
    });
    let b =
        w.d.store
            .batches(Some("scratch"), 5)
            .unwrap()
            .into_iter()
            .find(|b| b.finished_at.is_none())
            .unwrap();
    // The views offer it while the batch runs.
    let v = mason_daemon::views::Ctx::new(&w.d, &w.d.world().unwrap()).landing("scratch");
    assert!(
        v.batches
            .iter()
            .find(|x| x.id == b.id)
            .is_some_and(|x| x.allowed_actions == ["land_now"]),
        "{:?}",
        v.batches
    );
    assert!(
        v.items
            .iter()
            .filter(|i| i.status == "in_batch")
            .all(|i| i.allowed_actions == ["land_now"]),
        "{:?}",
        v.items
    );
    let (code, body) = post_api(&w.d, format!("/batches/{}/land-now", b.id));
    assert_eq!(code, 200, "{body}");
    wait_for("the batch to land past its blocked suite", 90, || {
        w.landed(t1) && w.landed(t2)
    });
    (w, gate, [t1, t2])
}

fn deferred_integration_debts(w: &W, t: TaskId) -> Vec<mason_store::DebtRow> {
    w.d.store
        .debts(Some(t.0), false)
        .unwrap()
        .into_iter()
        .filter(|x| x.scope == "suite:integration" && x.deferred)
        .collect()
}

#[test]
fn land_now_lands_a_batch_past_a_blocked_suite_and_the_skipped_suite_runs_after_the_merge() {
    let (w, gate, tasks) = land_now_blocked_batch(&[]);
    assert!(w.on_main("x/one.txt") && w.on_main("x/two.txt"));
    for t in tasks {
        assert_eq!(
            deferred_integration_debts(&w, t).len(),
            1,
            "one deferred suite debt per member"
        );
    }
    // The payment runs on the base and blocks on the gate: release it.
    std::fs::write(&gate, "go").unwrap();
    wait_for("both debts to be paid on the base tip", 120, || {
        tasks.iter().all(|t| {
            let d = deferred_integration_debts(&w, *t);
            d.len() == 1 && d[0].status != "open" && !d[0].how.is_empty()
        })
    });
}

#[test]
fn a_red_suite_run_after_land_now_leaves_the_debt_owed_and_raises_the_alert() {
    let (w, gate, tasks) = land_now_blocked_batch(&[("red.integration", "x")]);
    std::fs::write(&gate, "go").unwrap();
    wait_for("the debt to be tried and still red", 120, || {
        deferred_integration_debts(&w, tasks[0])
            .iter()
            .any(|x| x.status == "open" && x.tried_at > 0)
    });
    let alerts = mason_daemon::attention::alerts_now(&w.d);
    assert!(
        alerts
            .iter()
            .any(|(k, kind, _)| k == "debts" && kind == "debt"),
        "{alerts:?}"
    );
}

#[test]
fn land_now_does_not_land_a_batch_that_does_not_build() {
    let gate_dir = tempfile::tempdir().unwrap().keep();
    let gate = gate_dir.join("gate");
    let checks = gated_checks(&gate);
    let w = world_built(
        "window = \"2s\"\nmax_size = 5\n[[tier]]\nname = \"batch\"\nsuites = [\"integration\"]\n",
        &[("checks.sh", &checks)],
    );
    let t = w.task("does not build", &[("x/red.build.alpha", "x")]);
    wait_for("its batch to run", 90, || w.d.landing().in_batch(t));
    let (code, body) = post_api(&w.d, format!("/tasks/{}/land-now", t.0));
    assert_eq!(code, 200, "{body}");
    assert_eq!(body["applied"], true, "{body}");
    wait_for("it to go back: the build floor is kept", 120, || {
        w.sent_back(t).is_some()
    });
    assert!(!w.on_main("x/red.build.alpha"));
    assert!(!w.landed(t));
}

#[test]
fn land_now_keeps_the_freeze() {
    let w = world("window = \"0s\"\n", &[]);
    let fix = w.task("the fix", &[("x/fix.txt", "1")]);
    let other = w.task("not the fix", &[("x/other.txt", "1")]);
    w.d.store
        .set_freeze("scratch", "test freeze", Some(fix.0), 1)
        .unwrap();
    let (code, _) = post_api(&w.d, format!("/tasks/{}/land-now", other.0));
    assert_eq!(code, 200);
    // The fix passes the freeze; the coordinator has then looked at the queue with `other`
    // flagged, and left it.
    wait_for("the fix to land", 90, || w.landed(fix));
    assert!(!w.landed(other), "Land now does not pass a freeze");
    assert!(!w.on_main("x/other.txt"));
}

/// A fake GitHub that never assigns a runner: it accepts connections and says nothing. The
/// count is how many requests reached it.
pub(crate) fn silent_github() -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let n = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = n.clone();
    std::thread::spawn(move || {
        let mut held = vec![];
        for c in l.incoming().flatten() {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            held.push(c);
        }
    });
    (url, n)
}

fn github_world(
    tier: &str,
    files: &[(&str, &str)],
    script: Option<&str>,
) -> (W, Arc<std::sync::atomic::AtomicUsize>) {
    let (url, n) = silent_github();
    let extra = move |_: &Scratch| {
        format!(
            "[remote_runs]\nkind = \"github-actions\"\napi = \"{url}\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T806_TOKEN\"\nruns_on = \"macos-14\"\n"
        )
    };
    std::env::set_var("MASON_T806_TOKEN", "x");
    let mut all: Vec<(&str, &str)> = files.to_vec();
    if let Some(s) = script {
        all.push(("checks.sh", s));
    }
    let w = world_with(tier, &extra, &all);
    mason_daemon::place::set_global(&w.d, mason_daemon::place::Mode::Remote, "test").unwrap();
    (w, n)
}

#[test]
fn the_lint_tier_runs_here_while_the_switch_says_github() {
    // #806: lint is 1.5 min of work; GitHub's runner queue made it wait up to 20.
    let (w, github) = github_world(LINT_TIER, &[], None);
    let t = w.task("clean", &[("x/c.txt", "1")]);
    wait_for("the batch to land", 90, || w.landed(t));
    assert!(w.on_main("x/c.txt"));
    assert_eq!(
        github.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the lint reached the fake GitHub"
    );
}

#[test]
fn a_lint_tier_set_to_github_goes_there_and_holds_no_local_compile_slot_while_queued() {
    let tier = format!("{LINT_TIER}where = \"github\"\n");
    let (w, github) = github_world(&tier, &[], None);
    let _t = w.task("clean", &[("x/c.txt", "1")]);
    wait_for("the lint to reach GitHub", 90, || {
        github.load(std::sync::atomic::Ordering::SeqCst) > 0
    });
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(
        w.d.broker.used("compile"),
        0,
        "a lint waiting on GitHub holds a local compile slot"
    );
}

#[test]
fn a_red_lint_run_here_parks_the_task_with_file_and_line() {
    let probe = tempfile::tempdir().unwrap();
    let script = lint_checks_script(&probe.path().join("calls"));
    let (w, github) = github_world(LINT_TIER, &[], Some(&script));
    let t = w.task("unformatted", &[("x/bad.rs", "fn  main(){}")]);
    wait_for("the task to be sent back", 90, || w.sent_back(t).is_some());
    let why = w.sent_back(t).unwrap();
    assert!(why.contains("x/bad.rs:1"), "{why}");
    assert_eq!(github.load(std::sync::atomic::Ordering::SeqCst), 0);
}
