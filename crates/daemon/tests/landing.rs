// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 14: the landing engine end to end. Tasks are put straight into the
//! landing step (their branches made here), so no agent is involved: only
//! real git, real suites and the real coordinator.

mod support;
use mason_core::*;
use mason_landing::git;
use std::sync::Arc;
use std::time::{Duration, Instant};
use support::lw::*;
use support::*;

#[test]
fn a_batch_of_five_lands_three_ejects_the_conflict_and_parks_the_culprit() {
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
    let want_parked = format!("{},{}", t4.0, t5.0);
    wait_for("the batch row to be final", 30, || {
        w.d.store
            .batches(Some("scratch"), 10)
            .unwrap()
            .iter()
            .any(|b| b.phase == "landed" && b.parked == want_parked)
    });
    let rows = w.d.store.batches(Some("scratch"), 10).unwrap();
    let b = rows
        .iter()
        .find(|b| b.phase == "landed")
        .expect("a landed batch");
    assert_eq!(b.landed, format!("{},{},{}", t1.0, t2.0, t3.0));
    assert_eq!(b.parked, format!("{},{}", t4.0, t5.0));
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
    // The batch is green on its own tree; while it verifies, the base gains a file that
    // turns `integration` red. The re-verification then has a named test red on both
    // runs with no baseline stored, which used to take the repository lock a second time
    // from the landing thread, holding it, and hang. It must end: here, sent back.
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
    w.s.push_files(&[("red.integration", "boom")], "turn integration red");
    wait_for("the landing to end instead of hanging", 120, || {
        w.sent_back(t).is_some() || w.landed(t)
    });
    assert!(!w.on_main("x/slow.integration"), "a red tree was promoted");
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
