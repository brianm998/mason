// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 21: proposals and self-improvement. Duplicates and evidence-free filings are refused,
//! filings are capped, policy and routing decide where a proposal goes, the owner can approve,
//! reject and edit, and MASON can improve itself: with write access by the normal landing, and
//! without it on a fork branch with a channel switch, an upstream pull request only after a
//! redacted preview and a confirmation, and a way back when it merges or is turned down.

mod support;
use mason_core::*;
use mason_daemon::{improve, proposals, Daemon};
use std::path::PathBuf;
use std::sync::Arc;
use support::lw::*;
use support::*;

const CLIENT: &str = "acmeshop";
const MASON_FILE: &str = "crates/engine/src/lib.rs";
const CLIENT_FILE: &str = "app/Pay.swift";

struct P {
    w: W,
    client: TaskId,
    client_dir: PathBuf,
}

fn pid(d: &Arc<Daemon>, name: &str) -> ProjectId {
    d.project_id_by_name(name).unwrap()
}

/// MASON's own repository is the project `scratch`; a client project `acmeshop` has a repository
/// of its own with a file of its own. `self_extra` and `client_extra` add to their project files.
fn setup(self_extra: &str, client_extra: &str, host: bool) -> P {
    let dir: std::cell::RefCell<PathBuf> = Default::default();
    let (se, ce) = (self_extra.to_string(), client_extra.to_string());
    let w = world_with(
        "mode = \"direct\"\nwindow = \"0s\"\n",
        &|s| {
            let client = s.root.path().join("acme-code");
            std::fs::create_dir_all(client.join("app")).unwrap();
            sh(&client, &["init", "-q", "-b", "main"]);
            std::fs::write(client.join(CLIENT_FILE), "struct Pay {}\n").unwrap();
            std::fs::write(client.join("README"), "secret client code\n").unwrap();
            sh(&client, &["add", "-A"]);
            sh(
                &client,
                &[
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "commit",
                    "-qm",
                    "init",
                ],
            );
            std::fs::write(
                s.config_dir.join("projects").join(format!("{CLIENT}.toml")),
                format!(
                    "path = \"{}\"\nbase = \"main\"\ntest = \"true\"\n{ce}\n",
                    client.display()
                ),
            )
            .unwrap();
            *dir.borrow_mut() = client;
            let hostsec = if host {
                host_section(s, false, false)
            } else {
                String::new()
            };
            let se = se.replace("__ORIGIN__", &s.origin.display().to_string());
            format!("[self]\nmax_open = 50\nper_week = 50\nmax_steps = 500\n{se}\n{hostsec}")
        },
        &[(MASON_FILE, "// engine\n"), ("docs/notes.md", "notes\n")],
    );
    // The checkout is on its own branch; give it the files the evidence will cite.
    std::fs::create_dir_all(w.s.repo.join("crates/engine/src")).unwrap();
    std::fs::write(w.s.repo.join(MASON_FILE), "// engine\n").unwrap();
    let client = emit_task(&w.d, CLIENT, "work on the checkout");
    let client_dir = dir.into_inner();
    P {
        w,
        client,
        client_dir,
    }
}

fn emit_task(d: &Arc<Daemon>, project: &str, title: &str) -> TaskId {
    let a = d
        .emit(EventKind::TaskFiled {
            project: pid(d, project),
            title: title.into(),
            body: "x".into(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: true,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .unwrap();
    a.effects
        .iter()
        .find_map(|e| {
            if let Effect::TaskCreated { task } = e {
                Some(*task)
            } else {
                None
            }
        })
        .unwrap()
}

fn filing(
    from: TaskId,
    project: ProjectId,
    title: &str,
    evidence: &str,
    blocks: bool,
) -> proposals::Filing {
    proposals::Filing {
        from_task: Some(from),
        project,
        title: title.into(),
        body: format!("{title}: the problem in detail, and what to do about it."),
        evidence: evidence.into(),
        blocks_task: blocks,
        explored: false,
    }
}

fn task_count(d: &Arc<Daemon>) -> usize {
    d.world().unwrap().tasks.len()
}

#[test]
fn duplicates_and_evidence_free_proposals_are_refused_and_file_nothing() {
    let p = setup("", "", false);
    let (d, c) = (&p.w.d, pid(&p.w.d, CLIENT));
    let ev = format!("{CLIENT_FILE}:42 charges the card twice when tapped quickly");
    let first = proposals::file(
        d,
        &filing(p.client, c, "Pay button charges twice", &ev, false),
    )
    .unwrap();
    let n = task_count(d);

    let dup = proposals::file(
        d,
        &filing(p.client, c, "pay button charges twice!", &ev, false),
    )
    .unwrap_err();
    assert!(
        dup.contains(&format!("duplicate of #{}", first.task.0)),
        "{dup}"
    );
    for bad in [
        "",
        "   \n\t ",
        "it looks broken to me somehow, really quite broken",
        "short.rs",
    ] {
        let e = proposals::file(
            d,
            &filing(p.client, c, "A different problem entirely", bad, false),
        )
        .unwrap_err();
        assert!(e.contains("evidence"), "{bad:?}: {e}");
    }
    assert_eq!(task_count(d), n, "nothing was filed by a refusal");
}

#[test]
fn a_task_may_file_three_and_a_project_twenty_a_day() {
    let p = setup("", "[proposals]\nper_day = 4\n", false);
    let (d, c) = (&p.w.d, pid(&p.w.d, CLIENT));
    let ev = format!("{CLIENT_FILE}:1 shows the problem clearly enough");
    let titles = [
        "alpha widget leaks memory",
        "beta server drops packets",
        "gamma cache never expires",
        "delta parser rejects tabs",
    ];
    for t in &titles[..3] {
        proposals::file(d, &filing(p.client, c, t, &ev, false)).unwrap();
    }
    let e = proposals::file(d, &filing(p.client, c, titles[3], &ev, false)).unwrap_err();
    assert!(e.contains("limit is 3"), "{e}");
    // Another task may still file, until the project's day is used up.
    let other = emit_task(d, CLIENT, "another");
    proposals::file(d, &filing(other, c, titles[3], &ev, false)).unwrap();
    let third = emit_task(d, CLIENT, "third");
    let e = proposals::file(
        d,
        &filing(third, c, "epsilon logger blocks the thread", &ev, false),
    )
    .unwrap_err();
    assert!(e.contains("limit is 4") && e.contains("24 hours"), "{e}");
    // The defaults are 3 and 20.
    assert_eq!(
        mason_core::proposal::Caps::default(),
        mason_core::proposal::Caps {
            per_task: 3,
            per_day: 20
        }
    );
}

#[test]
fn policy_by_source_and_severity_decides_proposed_or_queued() {
    let p = setup("", "", false);
    let (d, c) = (&p.w.d, pid(&p.w.d, CLIENT));
    let ev = format!("{CLIENT_FILE}:7 the thing that was noticed");
    let a = proposals::file(
        d,
        &filing(p.client, c, "noticed something minor", &ev, false),
    )
    .unwrap();
    let b = proposals::file(
        d,
        &filing(p.client, c, "cannot finish without a fix", &ev, true),
    )
    .unwrap();
    assert!(a.proposed && !b.proposed);
    assert_eq!(state_of(d, a.task), State::Proposed);
    assert!(
        matches!(
            state_of(d, b.task),
            State::Queued | State::Exploring | State::Developing
        ),
        "{:?}",
        state_of(d, b.task)
    );
    assert_eq!(
        d.store.proposal(a.task.0).unwrap().unwrap().severity,
        "normal"
    );
    assert_eq!(
        d.store.proposal(b.task.0).unwrap().unwrap().severity,
        "blocks"
    );

    // The owner's table: everything an agent files runs, but what blocks still goes to the owner.
    let p = setup(
        "",
        "[proposals]\nagent_normal = \"queued\"\nagent_blocks = \"proposed\"\n",
        false,
    );
    let (d, c) = (&p.w.d, pid(&p.w.d, CLIENT));
    let a = proposals::file(
        d,
        &filing(p.client, c, "noticed something minor", &ev, false),
    )
    .unwrap();
    let b = proposals::file(
        d,
        &filing(p.client, c, "cannot finish without a fix", &ev, true),
    )
    .unwrap();
    assert!(!a.proposed && b.proposed);
    // A table that is not one is refused when the project loads.
    assert!(mason_daemon::config::parse_project(
        "x",
        "path=\"/x\"\nbase=\"main\"\ntest=\"t\"\n[proposals]\nagent_normal=\"maybe\"\n"
    )
    .is_err());
}

#[test]
fn a_proposal_is_routed_by_the_files_it_cites() {
    let p = setup("", "", false);
    let (d, c) = (&p.w.d, pid(&p.w.d, CLIENT));
    let own = proposals::file(
        d,
        &filing(
            p.client,
            c,
            "engine loses an event",
            &format!("{MASON_FILE}:12 drops the event on a full queue"),
            false,
        ),
    )
    .unwrap();
    assert_eq!((own.route, own.project.as_str()), ("pipeline", "scratch"));
    let task = d.world().unwrap().tasks[&own.task].project;
    assert_eq!(task, pid(d, "scratch"));
    let both = proposals::file(
        d,
        &filing(
            p.client,
            c,
            "checkout and engine disagree",
            &format!("{MASON_FILE}:12 and {CLIENT_FILE}:3 disagree"),
            false,
        ),
    )
    .unwrap();
    assert_eq!((both.route, both.project.as_str()), ("project", CLIENT));
    let client = proposals::file(
        d,
        &filing(
            p.client,
            c,
            "button label is wrong",
            &format!("{CLIENT_FILE}:3 says Pay, should say Buy"),
            false,
        ),
    )
    .unwrap();
    assert_eq!(client.route, "project");
    // Both are listed, with their route.
    let list = proposals::list_json(d, false);
    assert!(list
        .iter()
        .any(|r| r["route"] == "pipeline" && r["task"] == own.task.0));
    assert!(list
        .iter()
        .any(|r| r["route"] == "project" && r["task"] == client.task.0));
}

#[test]
fn the_owner_approves_rejects_and_edits_only_while_it_waits() {
    let p = setup("", "", false);
    let (d, c) = (&p.w.d, pid(&p.w.d, CLIENT));
    let ev = format!("{CLIENT_FILE}:9 flickers on every redraw");
    let a = proposals::file(
        d,
        &filing(p.client, c, "screen flickers on redraw", &ev, false),
    )
    .unwrap();
    assert!(proposals::edit(
        d,
        a.task.0,
        Some("screen flickers on every redraw"),
        None,
        Some("no locator here, just words and words")
    )
    .is_err());
    proposals::edit(
        d,
        a.task.0,
        Some("screen flickers on every redraw"),
        Some("new body"),
        Some(&format!("{CLIENT_FILE}:10 now line ten")),
    )
    .unwrap();
    let t = &d.world().unwrap().tasks[&a.task];
    assert_eq!(t.title, "screen flickers on every redraw");
    assert!(
        t.body.starts_with("new body") && t.body.contains("line ten"),
        "{}",
        t.body
    );
    let row = d.store.proposal(a.task.0).unwrap().unwrap();
    assert_eq!(
        (row.body.as_str(), row.source.as_str(), row.route.as_str()),
        ("new body", "agent", "project")
    );
    // An edit changes the dedupe key too.
    let e = proposals::file(
        d,
        &filing(p.client, c, "screen flickers on every redraw!", &ev, false),
    )
    .unwrap_err();
    assert!(e.contains("duplicate"));

    d.emit(EventKind::Approved { task: a.task }).unwrap();
    proposals::note_decision(d, a.task.0, true, "");
    assert_eq!(
        d.store.proposal(a.task.0).unwrap().unwrap().status,
        "approved"
    );
    let e = proposals::edit(d, a.task.0, Some("too late"), None, None).unwrap_err();
    assert_eq!(e.0, 409, "{e:?}");

    let b = proposals::file(
        d,
        &filing(p.client, c, "tooltip text is cut off", &ev, false),
    )
    .unwrap();
    d.emit(EventKind::Rejected { task: b.task }).unwrap();
    proposals::note_decision(d, b.task.0, false, "not worth it");
    let row = d.store.proposal(b.task.0).unwrap().unwrap();
    assert_eq!(
        (row.status.as_str(), row.reject_reason.as_str()),
        ("rejected", "not worth it")
    );
    // A rejected proposal is not filed again.
    let e = proposals::file(
        d,
        &filing(p.client, c, "tooltip text is cut off", &ev, false),
    )
    .unwrap_err();
    assert!(e.contains("rejected"), "{e}");
}

#[test]
fn an_improvement_with_write_access_is_an_owner_task_in_the_self_project() {
    let p = setup("", "", false);
    let d = &p.w.d;
    let (t, branch) = improve::improve(
        d,
        "scratch",
        "Make the log lines shorter\n\nThey wrap on a narrow screen.",
    )
    .unwrap();
    assert_eq!(branch, format!("mason/scratch/t{}", t.0));
    let row = d.store.proposal(t.0).unwrap().unwrap();
    assert_eq!(
        (row.source.as_str(), row.route.as_str(), row.access.as_str()),
        ("owner", "pipeline", "write")
    );
    let task = &d.world().unwrap().tasks[&t];
    assert_eq!(task.trust, TrustSource::Owner);
    assert_ne!(task.state, State::Proposed, "the owner's own request runs");
    assert!(
        improve::improve(d, CLIENT, "x").is_err(),
        "only MASON's own repository"
    );
    assert!(improve::improve(d, "scratch", "  ").is_err());
}

// ---- without write access ------------------------------------------------------------------

// Signed for the reason given on `BUILD` in tests/update.rs.
const BUILD: &str = "mkdir -p target/release\nfor b in masond step-host mason-mcp fake-claude; do cp \"$BINDIR/$b\" target/release/$b; ! command -v codesign >/dev/null || codesign -s - -f target/release/$b; done\n";

fn bindir() -> PathBuf {
    let m = bin("mason-daemon", "masond");
    bin("mason-step-host", "step-host");
    bin("mason-adapters", "mason-mcp");
    bin("mason-adapters", "fake-claude");
    m.parent().unwrap().to_path_buf()
}

fn update_toml(p: &P) {
    std::fs::write(
        p.w.s.config_dir.join("update.toml"),
        format!(
            "repo = \"{}\"\nchannel = \"stable\"\nbuild = {:?}\nbins = [\"masond\", \"step-host\", \"mason-mcp\", \"fake-claude\"]\n",
            p.w.s.origin.display(),
            format!("BINDIR='{}'\n{BUILD}", bindir().display())
        ),
    )
    .unwrap();
}

/// The fork is the bare repository the tests call `origin`: the fake host keeps every branch,
/// pull requests' heads included, in the one repository it is given, so the "fork" is that one.
fn fork_world() -> P {
    setup(
        "access = \"fork\"\nfork = \"__ORIGIN__\"\nswitch = \"check\"\n",
        "",
        true,
    )
}

fn channel(p: &P) -> String {
    let t = std::fs::read_to_string(p.w.s.config_dir.join("update.toml")).unwrap();
    t.lines()
        .find_map(|l| l.strip_prefix("channel = "))
        .unwrap()
        .trim_matches('"')
        .to_string()
}

/// An improvement already in its landing step, with the fork's row and a client's name and path
/// planted in its text.
fn fork_task(p: &P) -> (TaskId, String) {
    fork_task_changing(p, "docs/shorter.md")
}

fn fork_task_changing(p: &P, file: &str) -> (TaskId, String) {
    let d = &p.w.d;
    let (t, _) = improve::improve(d, "scratch", "Shorten the log lines").unwrap();
    // Use a task already in landing, as the landing tests do, and give it the improvement's row.
    let lt = p.w.task("Shorten the log lines", &[(file, "shorter\n")]);
    let mut row = d.store.proposal(t.0).unwrap().unwrap();
    row.task = lt.0;
    row.branch = format!("mason/improve/{}", lt.0);
    row.body = format!(
        "Shorten the log lines. Seen in {CLIENT} at {} and /Users/me/{CLIENT}/app/Pay.swift\n```\nlet payment = chargeCard()\n```",
        p.client_dir.display()
    );
    row.evidence = format!("{MASON_FILE}:3 wraps at 80 columns");
    d.store.put_proposal(&row).unwrap();
    let _ = d.emit(EventKind::Canceled {
        task: t,
        delete_branch: false,
    });
    (lt, row.branch)
}

fn assert_clean(what: &str, text: &str, p: &P) {
    for planted in [
        CLIENT,
        "/Users/me",
        "chargeCard",
        &p.client_dir.display().to_string(),
    ] {
        assert!(!text.contains(planted), "{planted} in {what}: {text}");
    }
}

#[test]
fn fork_access_needs_a_fork() {
    let r = mason_daemon::config::parse_project(
        "x",
        "path=\"/x\"\nbase=\"main\"\ntest=\"t\"\n[self]\naccess=\"fork\"\n",
    );
    assert!(r.unwrap_err().contains("fork"));
}

#[test]
fn a_fork_improvement_runs_on_its_branch_and_goes_upstream_only_redacted_and_confirmed() {
    let p = fork_world();
    let d = &p.w.d;
    update_toml(&p);
    let (lt, branch) = fork_task(&p);
    wait_for("the branch to land on the fork", 90, || p.w.landed(lt));
    assert!(
        !p.w.on_main("docs/shorter.md"),
        "nothing reached the repository's main"
    );
    assert!(
        mason_landing::git::ok(
            &p.w.s.origin,
            &["cat-file", "-e", &format!("{branch}:docs/shorter.md")]
        ),
        "the branch is on the fork"
    );
    wait_for("the channel to switch", 180, || {
        channel(&p) == format!("branch:{branch}")
    });
    assert_eq!(
        improve::switched(d).unwrap()["channel"],
        "stable",
        "the way back is remembered"
    );

    // Nothing goes upstream without a preview and a confirmation.
    let e = improve::publish(d, lt.0, true).unwrap_err();
    assert!(e.1.contains("--preview"), "{e:?}");
    let v = improve::publish(d, lt.0, false).unwrap();
    assert_clean("the preview", &v.to_string(), &p);
    assert_eq!(v["attachments"], serde_json::json!([]));
    assert_eq!(
        v["files"],
        serde_json::json!(["docs/shorter.md"]),
        "only MASON's own file"
    );
    assert!(v["redactions"].as_u64().unwrap() >= 3, "{v}");
    assert!(v["blocked"].is_null(), "{v}");
    assert!(
        d.store.proposal(lt.0).unwrap().unwrap().pr.is_none(),
        "a preview sends nothing"
    );

    // Confirmed: the pull request carries exactly the previewed, redacted text.
    let sent = improve::publish(d, lt.0, true).unwrap();
    let n = sent["pr"].as_u64().unwrap();
    let pr: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(p.w.s.root.path().join(format!("host-state/prs/{n}.json")))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (pr["title"].clone(), pr["body"].clone()),
        (v["title"].clone(), v["body"].clone())
    );
    assert_clean("the pull request", &pr.to_string(), &p);
    assert_eq!(
        d.store.proposal(lt.0).unwrap().unwrap().publish_state,
        "published"
    );
    assert!(improve::publish(d, lt.0, true).is_err(), "only once");
}

#[test]
fn a_branch_that_mentions_a_client_is_not_published() {
    let p = fork_world();
    let d = &p.w.d;
    let (lt, _) = fork_task(&p);
    // The branch's own change carries the client's code.
    let bad = p.w.task(
        "Fix",
        &[(
            "docs/notes.md",
            &format!("copied from {CLIENT}: struct Pay {{}}\n"),
        )],
    );
    let mut row = d.store.proposal(lt.0).unwrap().unwrap();
    row.task = bad.0;
    row.branch = format!("mason/improve/{}", bad.0);
    d.store.put_proposal(&row).unwrap();
    let v = improve::publish(d, bad.0, false).unwrap();
    assert!(v["blocked"].as_str().unwrap().contains("client"), "{v}");
    assert_clean("the preview", &v.to_string(), &p);
    assert!(improve::publish(d, bad.0, true).is_err());
}

#[test]
fn a_merged_pull_request_returns_the_channel_to_main() {
    let p = fork_world();
    let d = &p.w.d;
    update_toml(&p);
    let (lt, branch) = fork_task(&p);
    wait_for("the channel to switch", 180, || {
        channel(&p) == format!("branch:{branch}")
    });
    improve::publish(d, lt.0, false).unwrap();
    let n = improve::publish(d, lt.0, true).unwrap()["pr"]
        .as_u64()
        .unwrap();
    improve::poll(d);
    assert_eq!(
        channel(&p),
        format!("branch:{branch}"),
        "still open: still on the branch"
    );
    let host = mason_daemon::services::host(d.project(pid(d, "scratch")).unwrap())
        .unwrap()
        .unwrap();
    host.merge_pr(n).unwrap();
    improve::poll(d);
    assert_eq!(channel(&p), "main");
    assert!(improve::switched(d).is_none());
    assert_eq!(d.store.proposal(lt.0).unwrap().unwrap().pr_state, "merged");
    assert!(
        d.store
            .proposals()
            .unwrap()
            .iter()
            .all(|r| !r.key.starts_with("carry:")),
        "nothing to ask"
    );
}

#[test]
fn a_rejected_pull_request_asks_about_carrying_the_patch() {
    let p = fork_world();
    let d = &p.w.d;
    update_toml(&p);
    let (lt, branch) = fork_task(&p);
    wait_for("the channel to switch", 180, || {
        channel(&p) == format!("branch:{branch}")
    });
    improve::publish(d, lt.0, false).unwrap();
    let n = improve::publish(d, lt.0, true).unwrap()["pr"]
        .as_u64()
        .unwrap();
    let host = mason_daemon::services::host(d.project(pid(d, "scratch")).unwrap())
        .unwrap()
        .unwrap();
    host.close_pr(n).unwrap();
    improve::poll(d);
    improve::poll(d); // asked once
    let carry: Vec<_> = d
        .store
        .proposals()
        .unwrap()
        .into_iter()
        .filter(|r| r.key.starts_with("carry:"))
        .collect();
    assert_eq!(carry.len(), 1);
    assert_eq!(state_of(d, TaskId(carry[0].task)), State::Proposed);
    assert_eq!(
        channel(&p),
        format!("branch:{branch}"),
        "kept until the owner says"
    );
    // Approving keeps carrying it; rejecting rejoins main.
    d.emit(EventKind::Rejected {
        task: TaskId(carry[0].task),
    })
    .unwrap();
    proposals::note_decision(d, carry[0].task, false, "not worth carrying");
    assert_eq!(channel(&p), "main");
}

#[test]
fn a_branch_that_no_longer_merges_asks_the_same_question() {
    let p = fork_world();
    let d = &p.w.d;
    update_toml(&p);
    let (lt, branch) = fork_task_changing(&p, "docs/notes.md");
    wait_for("the channel to switch", 180, || {
        channel(&p) == format!("branch:{branch}")
    });
    improve::publish(d, lt.0, false).unwrap();
    improve::publish(d, lt.0, true).unwrap();
    // Upstream moves the same file another way.
    p.w.s
        .push_files(&[("docs/notes.md", "upstream's own change\n")], "upstream");
    // The task's local branch is what the conflict check reads; keep it for it.
    sh(&p.w.s.repo, &["fetch", "-q", "origin"]);
    improve::poll(d);
    assert_eq!(
        d.store.proposal(lt.0).unwrap().unwrap().pr_state,
        "conflict"
    );
    assert!(d
        .store
        .proposals()
        .unwrap()
        .iter()
        .any(|r| r.key == format!("carry:{}", lt.0)));
}
