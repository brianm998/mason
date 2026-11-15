// SPDX-License-Identifier: GPL-3.0-or-later

//! A fake agent that keeps committing is not stopped at its budget, and is stopped at the cap (s401).
//! No assertion measures time: the grants are counted.

use mason_adapter_api::contract::{ContractHandle, NoChecks};
use mason_adapter_api::{SessionSpec, StepKind};
use mason_adapters::fake::{FakeAction as A, FakeAdapter};
use mason_adapters::runner::*;
use mason_daemon::deadline::Extender;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn run_with(commits: bool) -> (StepEnd, Duration) {
    let budget = Duration::from_millis(400);
    let agent = FakeAdapter::new(vec![vec![A::Sleep(Duration::from_secs(60))]]);
    let c = ContractHandle::new(StepKind::Develop, Box::new(NoChecks));
    let mut spec = SessionSpec::new(StepKind::Develop, "p", std::env::temp_dir());
    spec.deadline = Some(budget);
    let (extra, stop) = (
        spec.deadline_extra.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let (extra2, stop2) = (extra.clone(), stop.clone());
    let t = std::thread::spawn(move || {
        let start = Instant::now();
        let mut ext = Extender::new(budget, budget, "c0".into(), 0);
        let mut n = 0u32;
        while !stop2.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(5));
            n += 1;
            // A committing agent has a new head at every look; a stuck one never does.
            ext.tick(
                start.elapsed(),
                &extra2,
                || {
                    if commits {
                        format!("c{n}")
                    } else {
                        "c0".into()
                    }
                },
                0,
            );
        }
    });
    let r = StepRunner::new(&agent, RunnerConfig::default()).run(spec, &c);
    stop.store(true, Ordering::SeqCst);
    t.join().unwrap();
    (r.end, extra.progressed())
}

#[test]
fn a_committing_agent_runs_past_the_budget_and_is_stopped_at_the_cap() {
    let (end, granted) = run_with(true);
    assert_eq!(end, StepEnd::Failed(FailReason::Timeout));
    assert_eq!(
        granted,
        Duration::from_millis(400),
        "the whole cap, no more"
    );
}

#[test]
fn an_agent_that_never_commits_gets_nothing_extra() {
    let (end, granted) = run_with(false);
    assert_eq!(end, StepEnd::Failed(FailReason::Timeout));
    assert_eq!(granted, Duration::ZERO);
}
