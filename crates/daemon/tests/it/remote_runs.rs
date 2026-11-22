// SPDX-License-Identifier: GPL-3.0-or-later

//! The home view's GitHub section (#767): every remote run with its owner, the owner's state,
//! its kind, where it stands, its jobs and its URL. The jobs are put in the registry where the
//! run's poll puts them; the debt run's jobs are read from a recorded GitHub answer (the jobs
//! API of the run) and turned into the registry's view by the poll's own `job_view`.

use crate::support::*;
use mason_core::*;
use mason_daemon::remote::{spec_for, HoldGuard};
use mason_daemon::remote_runs;
use std::time::Duration;

fn record(dir: &std::path::Path, id: &str, run: u64, picked: Option<u64>) {
    std::fs::write(
        dir.join(format!("{id}.remote")),
        format!(
            "sha=abc\nref=mason/run/scratch/{id}\nrun_id={run}\nurl=https://github.com/o/r/actions/runs/{run}\npicked_ms={}\n",
            picked.map(|p| p.to_string()).unwrap_or_default()
        ),
    )
    .unwrap();
}

#[test]
fn the_home_lists_each_run_with_its_owner_state_and_jobs() {
    let s = Scratch::new("true", happy_script());
    s.write_project(
        "scratch",
        "true",
        "[remote_runs]\nkind = \"github-actions\"\nrepo = \"o/r\"\ntoken = \"env:MASON_T767_TOKEN\"\n",
    );
    let d = s.start();
    // A fresh store numbers tasks from 0, and task 0 is how a run says "no task" (a landing
    // batch): file one first so the tasks below have real ids.
    let _zero = file_task_as(&d, "Zero", "Do nothing.", true, TrustSource::Owner, false);
    let a = file_task_as(&d, "Check me", "Do a.", true, TrustSource::Owner, false);
    let b = file_task_as(&d, "Pay me", "Do b.", true, TrustSource::Owner, false);
    let c = file_task_as(&d, "Wait for me", "Do c.", true, TrustSource::Owner, false);
    let logs = tempfile::tempdir().unwrap();
    let hosts = d.cfg.hosts_dir();
    let spec = |id: &str, shards: u32| {
        let mut r = spec_for(
            "scratch",
            id,
            None,
            "true",
            Duration::from_secs(60),
            &[],
            "ubuntu-latest",
            "",
        );
        r.shards = shards;
        r
    };
    // One check, pushed and not yet taken by a runner.
    let ia = format!("t{}r1-check0", a.0);
    record(logs.path(), &ia, 11, None);
    let _check = HoldGuard::new(
        &hosts,
        &spec(&ia, 1),
        &ia,
        &ia,
        1,
        logs.path(),
        "check",
        a.0,
    );
    // One sharded debt run: 8 jobs, 3 done.
    let ib = format!("t{}r2-debt", b.0);
    record(logs.path(), &ib, 12, Some(mason_daemon::daemon::now_ms()));
    let _debt = HoldGuard::new(
        &hosts,
        &spec(&ib, 8),
        &format!("task {}", b.0),
        &ib,
        8,
        logs.path(),
        "debt",
        b.0,
    );
    let body: Vec<serde_json::Value> = (1..=8)
        .map(|k| {
            if k <= 3 {
                serde_json::json!({"name": format!("run ({k})"), "status": "completed",
                                   "conclusion": "success"})
            } else {
                serde_json::json!({"name": format!("run ({k})"), "status": "in_progress",
                                   "steps": [{"name": "run", "status": "in_progress"}]})
            }
        })
        .collect();
    let tok = logs.path().join("token");
    std::fs::write(&tok, "ghp_test_token\n").unwrap();
    let rec = serde_json::json!([{"method": "GET", "url": "/actions/runs/12/jobs",
        "response": {"status": 200, "body": {"total_count": 8, "jobs": body}}}]);
    let gh = mason_adapters::github::GitHub::new(
        std::sync::Arc::new(
            mason_adapter_api::http::Recorded::from_json(&rec.to_string()).unwrap(),
        ),
        "https://api.github.com",
        "o/r",
        mason_adapters::github::TokenRef::File(tok),
    );
    let jobs = mason_daemon::remote::job_view(&gh.remote_run_jobs(12).unwrap());
    remote_runs::set_jobs(&logs.path().join(format!("{ib}.remote")), jobs);
    // One landing lint out, and a step waiting for a pool unit.
    let il = "b480-r1-lint".to_string();
    record(logs.path(), &il, 13, None);
    let _lint = HoldGuard::new(&hosts, &spec(&il, 1), &il, &il, 1, logs.path(), "batch", 0);
    let _wait = remote_runs::waiting(&d, c.0, "test step", &format!("#{}", c.0));

    let world = d.world().unwrap();
    let home = mason_daemon::views::Ctx::new(&d, &world).home("");
    let g = home.github.expect("the GitHub section");
    assert!(g.enabled);
    assert_eq!(g.runs.len(), 4, "{:?}", g.runs);
    assert_eq!(g.waiting, 1);
    let by = |owner: &str| g.runs.iter().find(|r| r.owner == owner).unwrap().clone();

    let check = by(&format!("#{}", a.0));
    assert_eq!(
        (check.kind.as_str(), check.state.as_str()),
        ("agent check", "queued")
    );
    assert_eq!(check.task, a.0);
    assert_eq!(check.title, "Check me");
    assert_eq!(check.url, "https://github.com/o/r/actions/runs/11");

    let debt = by(&format!("debt #{}", b.0));
    assert_eq!(
        (debt.kind.as_str(), debt.state.as_str()),
        ("debt payment", "running")
    );
    assert_eq!(debt.shards, 8);
    assert_eq!(debt.jobs.len(), 8);
    assert_eq!(debt.jobs.iter().filter(|j| j.state == "done").count(), 3);
    assert!(
        debt.progress.starts_with("3 of 8 jobs done"),
        "{}",
        debt.progress
    );
    assert!(debt.progress.contains("step run"), "{}", debt.progress);
    assert_eq!(debt.url, "https://github.com/o/r/actions/runs/12");

    let lint = by("b480");
    assert_eq!((lint.kind.as_str(), lint.task), ("landing lint", 0));
    assert_eq!(lint.url, "https://github.com/o/r/actions/runs/13");

    let wait = by(&format!("#{}", c.0));
    assert_eq!(wait.state, "waiting");
    assert!(wait.url.is_empty());
    // Waiting runs come after the ones that are out.
    assert_eq!(g.runs.last().unwrap().owner, wait.owner);
    // Every row names its task's state.
    assert!(!check.task_state.is_empty() && !wait.task_state.is_empty());

    // A run that ends stays, with how it ended.
    drop(_lint);
    let home = mason_daemon::views::Ctx::new(&d, &world).home("");
    let g = home.github.unwrap();
    let lint = g.runs.iter().find(|r| r.owner == "b480").unwrap();
    assert_eq!(lint.state, "cancelled");
}
