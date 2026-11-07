// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 17: MASON on its own repository. A change to a gate, guard, budget,
//! verdict or trust file stops for the owner; so does one that weakens a check
//! anywhere else; a harmless one lands freely; and self-tasks are limited.

mod support;
use mason_core::*;
use mason_daemon::selfguard;
use std::time::Duration;
use support::lw::*;
use support::*;

const MATRIX: &str = "#[test]\nfn pairs() {\n    assert_eq!(a, 1);\n    assert_eq!(b, 2);\n}\n";
const WS: &str = "const RESUME_LIMIT: u64 = 2000;\nfn f() {}\n";

fn self_world(extra: &str) -> W {
    let extra = extra.to_string();
    world_with(
        "mode = \"direct\"\nwindow = \"0s\"\n",
        &move |_| format!("[self]\n{extra}"),
        &[
            ("crates/engine/tests/matrix.rs", MATRIX),
            ("crates/daemon/src/ws.rs", WS),
            ("crates/daemon/src/protect.rs", "// the rules\n"),
            ("docs/notes.md", "notes\n"),
        ],
    )
}

fn owner(w: &W) -> String {
    w.d.cfg.owner_names[0].clone()
}

fn held(w: &W, t: TaskId) -> bool {
    w.d.store.hold(t.0).unwrap().is_some()
}

#[test]
fn a_harmless_change_to_its_own_repository_lands_freely() {
    let w = self_world("");
    let t = w.task("a note", &[("docs/notes.md", "more notes\n")]);
    wait_for("it to land", 60, || w.landed(t));
    assert!(!held(&w, t), "nothing was held");
}

#[test]
fn a_change_to_a_gate_waits_for_the_owner_and_only_the_owner_can_release_it() {
    let w = self_world("");
    let t = w.task(
        "edit a verdict",
        &[("crates/testing/src/verdict.rs", "// weaker\n")],
    );
    wait_for("the hold", 30, || held(&w, t));
    let hold = w.d.store.hold(t.0).unwrap().unwrap();
    assert!(
        hold.reasons.contains("crates/testing/src/verdict.rs")
            && hold.reasons.contains("verdict logic"),
        "{}",
        hold.reasons
    );
    // It says why it waits, and raises it as something for the owner.
    wait_for("the wait to show", 30, || {
        task_of(&w.d, t).activity != Activity::Running
    });
    let alerts = mason_daemon::attention::alerts_now(&w.d);
    assert!(
        alerts.iter().any(|(_, k, text)| k == "needs_you"
            && text.contains(&format!("Task #{}", t.0))
            && text.contains("mason task approve")),
        "{alerts:?}"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(!w.landed(t), "it must not land unapproved");
    // Someone else, and nobody, cannot approve.
    assert_eq!(selfguard::approve(&w.d, t, "mallory").unwrap_err().0, 403);
    assert_eq!(selfguard::approve(&w.d, t, "").unwrap_err().0, 403);
    assert!(!w.landed(t));
    // The owner can; it then lands, and the intervention is on record.
    assert!(selfguard::approve(&w.d, t, &owner(&w)).unwrap());
    wait_for("it to land", 60, || w.landed(t));
    assert_eq!(
        selfguard::approve(&w.d, t, &owner(&w)).unwrap_err().0,
        409,
        "approved once"
    );
    let alerts = mason_daemon::attention::alerts_now(&w.d);
    assert!(
        !alerts
            .iter()
            .any(|(_, _, text)| text.contains("waits for your approval")),
        "the alert cleared"
    );
}

#[test]
fn weakening_outside_the_protected_files_stops_too() {
    let w = self_world("");
    // A removed assertion in a test file nobody listed.
    let a = w.task(
        "tidy a test",
        &[(
            "crates/engine/tests/matrix.rs",
            "#[test]\nfn pairs() {\n    assert_eq!(a, 1);\n}\n",
        )],
    );
    // A raised limit in ordinary source.
    let b = w.task(
        "be generous",
        &[(
            "crates/daemon/src/ws.rs",
            "const RESUME_LIMIT: u64 = 2000000;\nfn f() {}\n",
        )],
    );
    wait_for("both held", 30, || held(&w, a) && held(&w, b));
    let ra = w.d.store.hold(a.0).unwrap().unwrap().reasons;
    let rb = w.d.store.hold(b.0).unwrap().unwrap().reasons;
    assert!(
        ra.contains("removed assertion") && ra.contains("crates/engine/tests/matrix.rs"),
        "{ra}"
    );
    assert!(
        rb.contains("raised limit") && rb.contains("resume_limit: 2000 -> 2000000"),
        "{rb}"
    );
    assert!(
        protect_none(&["crates/engine/tests/matrix.rs", "crates/daemon/src/ws.rs"]),
        "neither file is on the protected list"
    );
    std::thread::sleep(Duration::from_secs(2));
    assert!(!w.landed(a) && !w.landed(b));
    // The owner looks at the second and says no (by cancelling); the first is approved and lands.
    assert!(selfguard::approve(&w.d, a, &owner(&w)).unwrap());
    wait_for("the approved one to land", 60, || w.landed(a));
    assert!(!w.landed(b));
}

fn protect_none(paths: &[&str]) -> bool {
    paths
        .iter()
        .all(|p| mason_daemon::protect::protected_because(p, &[]).is_none())
}

#[test]
fn a_change_cannot_switch_off_the_check_on_itself() {
    let w = self_world("");
    // The candidate edits the rule set AND adds a file claiming nothing is protected: the
    // running version's compiled rules decide, so it is held regardless.
    let t = w.task(
        "turn the check off",
        &[
            ("crates/daemon/src/protect.rs", "// PROTECTED = []\n"),
            (".mason/protect.toml", "protected = []\n"),
        ],
    );
    wait_for("the hold", 30, || held(&w, t));
    let r = w.d.store.hold(t.0).unwrap().unwrap().reasons;
    assert!(
        r.contains("crates/daemon/src/protect.rs") && r.contains("self-change rules"),
        "{r}"
    );
    assert!(r.contains(".mason/protect.toml"), "{r}");
    std::thread::sleep(Duration::from_secs(2));
    assert!(!w.landed(t));
}

#[test]
fn the_owner_can_add_protected_paths_in_the_project_file() {
    let w = self_world("protect = [\"docs/secret/**\"]\n");
    let t = w.task("touch the secret", &[("docs/secret/plan.md", "x\n")]);
    wait_for("the hold", 30, || held(&w, t));
    assert!(w
        .d
        .store
        .hold(t.0)
        .unwrap()
        .unwrap()
        .reasons
        .contains("protected by the owner: docs/secret/**"));
}

#[test]
fn self_tasks_are_limited_in_number_per_week_and_by_a_cooldown() {
    let w = self_world("max_open = 2\nper_week = 3\n");
    let pid = w.d.project_id_by_name("scratch").unwrap();
    assert!(selfguard::check_filing(&w.d, pid).is_ok());
    // Two open tasks: the third is refused with the number and the limit.
    file_task_as(&w.d, "one", "x", true, TrustSource::Owner, true);
    file_task_as(&w.d, "two", "x", true, TrustSource::Owner, true);
    let e = selfguard::check_filing(&w.d, pid).unwrap_err();
    assert!(
        e.contains("2 self-tasks are open") && e.contains("limit is 2"),
        "{e}"
    );
    // A rollback starts a cooldown, whatever the counts.
    let w2 = self_world("");
    let pid2 = w2.d.project_id_by_name("scratch").unwrap();
    mason_daemon::update::Runtimes::new(&w2.d.cfg.data_dir).record_rollback(
        "aaaaaaaaaaaa",
        "bbbbbbbbbbbb",
        mason_daemon::daemon::now_ms(),
    );
    let e = selfguard::check_filing(&w2.d, pid2).unwrap_err();
    assert!(e.contains("cooling down after a rollback"), "{e}");
}

#[test]
fn a_rollback_files_one_proposal_for_the_owner_and_only_one() {
    let w = self_world("");
    let rt = mason_daemon::update::Runtimes::new(&w.d.cfg.data_dir);
    rt.record_rollback(
        "deadbeefdeadbeef",
        "cafebabecafebabe",
        mason_daemon::daemon::now_ms(),
    );
    selfguard::on_start(&w.d);
    selfguard::on_start(&w.d);
    let tasks: Vec<_> =
        w.d.world()
            .unwrap()
            .tasks
            .values()
            .filter(|t| t.title.starts_with("Investigate the rollback"))
            .cloned()
            .collect();
    assert_eq!(tasks.len(), 1, "{tasks:?}");
    let t = &tasks[0];
    assert_eq!(t.state, State::Proposed, "it waits for the owner");
    assert!(
        t.body.contains("deadbeefdeadbeef") && t.body.contains("cafebabecafebabe"),
        "{}",
        t.body
    );
    assert!(rt.rollback().unwrap().3, "marked filed");
}

#[test]
fn a_project_that_is_not_masons_own_has_no_holds_and_no_limits() {
    let w = world(
        "mode = \"direct\"\nwindow = \"0s\"\n",
        &[("crates/testing/src/verdict.rs", "x\n")],
    );
    let t = w.task("edit anything", &[("crates/testing/src/verdict.rs", "y\n")]);
    wait_for("it to land", 60, || w.landed(t));
    let pid = w.d.project_id_by_name("scratch").unwrap();
    assert!(selfguard::check_filing(&w.d, pid).is_ok());
}

// ---- Stage 41: which diff is read -----------------------------------------------------------

mod scan {
    use mason_daemon::selfguard::{change_in, scan_base};
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let o = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }

    fn put(dir: &Path, f: &str, text: &str) {
        let p = dir.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn commit(dir: &Path, f: &str, text: &str) {
        put(dir, f, text);
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", f]);
    }

    /// A clone with `origin/develop` and a local `develop`, both at the first commit.
    fn repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let t = tempfile::tempdir().unwrap();
        let (origin, work) = (t.path().join("origin.git"), t.path().join("work"));
        std::fs::create_dir_all(&work).unwrap();
        git(
            t.path(),
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "develop",
                origin.to_str().unwrap(),
            ],
        );
        git(&work, &["init", "-q", "-b", "develop"]);
        git(
            &work,
            &["remote", "add", "origin", origin.to_str().unwrap()],
        );
        commit(&work, "scripts/exit/s17.sh", "echo gate\n");
        commit(&work, "docs/a.md", "a\n");
        git(&work, &["push", "-q", "origin", "develop"]);
        (t, work)
    }

    fn paths(c: &mason_daemon::selfguard::Change) -> Vec<String> {
        c.files.iter().map(|f| f.path.clone()).collect()
    }

    #[test]
    fn a_base_merged_into_the_branch_does_not_count_as_the_tasks_change() {
        let (_t, w) = repo();
        git(&w, &["checkout", "-q", "-b", "task"]);
        commit(&w, "docs/task.md", "mine\n");
        // The base moves on with a protected change that lands (local develop and origin).
        git(&w, &["checkout", "-q", "develop"]);
        commit(&w, "scripts/exit/s20.sh", "kill $P || true\n");
        git(&w, &["push", "-q", "origin", "develop"]);
        git(&w, &["fetch", "-q", "origin"]);
        // The task merges the base in.
        git(&w, &["checkout", "-q", "task"]);
        git(&w, &["merge", "-q", "--no-edit", "develop"]);
        let c = change_in(&w, "origin/develop", "develop", "task").unwrap();
        assert_eq!(paths(&c), ["docs/task.md"], "{}", c.diff);
        assert!(!c.diff.contains("s20.sh"));
        assert!(c.range.ends_with("...task"));
    }

    #[test]
    fn a_stale_remote_ref_does_not_make_landed_work_the_tasks_own() {
        let (_t, w) = repo();
        git(&w, &["checkout", "-q", "-b", "task"]);
        commit(&w, "docs/task.md", "mine\n");
        // Local develop moves ahead of origin/develop (landed, not yet pushed or fetched).
        git(&w, &["checkout", "-q", "develop"]);
        commit(&w, "scripts/exit/s20.sh", "kill $P || true\n");
        git(&w, &["checkout", "-q", "task"]);
        git(&w, &["merge", "-q", "--no-edit", "develop"]);
        assert_eq!(scan_base(&w, "origin/develop", "develop"), "develop");
        let c = change_in(&w, "origin/develop", "develop", "task").unwrap();
        assert_eq!(paths(&c), ["docs/task.md"]);
        // Against the stale ref alone, the landed file would have been blamed on the task.
        let stale = change_in(&w, "origin/develop", "origin/develop", "task").unwrap();
        assert!(paths(&stale).contains(&"scripts/exit/s20.sh".to_string()));
    }

    #[test]
    fn a_siblings_unlanded_protected_change_merged_in_is_still_listed() {
        let (_t, w) = repo();
        git(&w, &["checkout", "-q", "-b", "sibling"]);
        commit(&w, "scripts/exit/s30.sh", "kill $P || true\n");
        git(&w, &["checkout", "-q", "develop"]);
        git(&w, &["checkout", "-q", "-b", "task"]);
        commit(&w, "docs/task.md", "mine\n");
        git(&w, &["merge", "-q", "--no-edit", "sibling"]);
        let c = change_in(&w, "origin/develop", "develop", "task").unwrap();
        let p = paths(&c);
        assert!(p.contains(&"scripts/exit/s30.sh".to_string()), "{p:?}");
        assert!(
            c.files
                .iter()
                .find(|f| f.path == "scripts/exit/s30.sh")
                .unwrap()
                .added
        );
    }

    #[test]
    fn an_edit_to_an_existing_exit_script_is_an_edit_and_names_its_line() {
        let (_t, w) = repo();
        git(&w, &["checkout", "-q", "-b", "task"]);
        commit(&w, "scripts/exit/s17.sh", "echo gate\ncargo test || true\n");
        commit(&w, "scripts/exit/s50.sh", "echo new\n");
        let c = change_in(&w, "origin/develop", "develop", "task").unwrap();
        let i = mason_daemon::protect::inspect_changed(&c.files, &c.diff, &[]);
        let text = i.short(&format!("git diff {}", c.range));
        assert!(text.contains("touches scripts/exit/s17.sh:2"), "{text}");
        assert!(
            text.contains("broadened skip at scripts/exit/s17.sh:2"),
            "{text}"
        );
        assert!(
            !text.contains("s50.sh"),
            "the new exit script is not held by path: {text}"
        );
    }
}
