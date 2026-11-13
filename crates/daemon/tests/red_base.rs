// SPDX-License-Identifier: GPL-3.0-or-later

//! A red landing batch against the base branch's own CI (s174, decision 0169): the base's
//! latest finished run of the same workflow is read through the `Source` the watch uses, and
//! the failing tests of the two logs are compared. No timing assertions.

use mason_adapter_api::service::JobFailure;
use mason_adapters::github::{ActionsJob, ActionsRun};
use mason_core::red_base::Verdict;
use mason_daemon::ci_watch::{assess, Source};
use std::collections::BTreeMap;

/// The base branch: runs newest first, the jobs of each, the log of each job.
struct Base {
    runs: Vec<ActionsRun>,
    jobs: BTreeMap<u64, Vec<ActionsJob>>,
    logs: BTreeMap<u64, String>,
}

impl Source for Base {
    fn runs(&self, _branch: &str) -> Result<Vec<ActionsRun>, String> {
        Ok(self.runs.clone())
    }
    fn jobs(&self, run: u64) -> Result<Vec<ActionsJob>, String> {
        Ok(self.jobs.get(&run).cloned().unwrap_or_default())
    }
    fn log_tail(&self, job: u64) -> Option<String> {
        self.logs.get(&job).cloned()
    }
}

const TEST: &str = "pipeline/tests/test_impact.py::test_no_top_level_source_directory_is_unmapped";

fn base(conclusion: &str, log: &str) -> Base {
    Base {
        runs: vec![ActionsRun {
            id: 7,
            workflow: "pr-gate".into(),
            sha: "a".repeat(40),
            url: String::new(),
            created_ms: 1,
            conclusion: Some(conclusion.into()),
        }],
        jobs: BTreeMap::from([(
            7,
            vec![ActionsJob {
                id: 71,
                name: "static checks".into(),
                conclusion: Some(conclusion.into()),
            }],
        )]),
        logs: BTreeMap::from([(71, log.to_string())]),
    }
}

fn batch(log: &str) -> Vec<JobFailure> {
    vec![JobFailure {
        workflow: "pr-gate".into(),
        job: "static checks".into(),
        infra: None,
        log: Some(log.into()),
        url: None,
    }]
}

fn failed(text: &str) -> String {
    format!("collecting\nFAILED {TEST} - AssertionError: {text}\n1 failed, 40 passed")
}

#[test]
fn the_same_test_red_on_the_base_blames_nobody() {
    let b = base("failure", &failed("['.mason']"));
    let v = assess(&b, "develop", &batch(&failed("['.mason']")));
    assert_eq!(v, Verdict::RedOnBase("pr-gate/static checks".into()));
}

#[test]
fn a_job_green_on_the_base_is_the_batchs() {
    let b = base("success", "all fine");
    assert_eq!(
        assess(&b, "develop", &batch(&failed("['.mason']"))),
        Verdict::Blame
    );
}

#[test]
fn a_new_failing_test_on_a_red_job_is_the_batchs() {
    let b = base("failure", &failed("['.mason']"));
    let log = format!(
        "{}\nFAILED tests/t.py::test_new - assert 0",
        failed("['.mason']")
    );
    assert_eq!(assess(&b, "develop", &batch(&log)), Verdict::Blame);
}

#[test]
fn the_same_test_with_other_text_is_held_with_both_texts() {
    let b = base("failure", &failed("['.mason']"));
    let v = assess(&b, "develop", &batch(&failed("['.mason', 'scripts']")));
    let Verdict::Differs(t) = v else {
        panic!("{v:?}")
    };
    assert!(
        t.contains("['.mason', 'scripts']") && t.contains("['.mason']"),
        "{t}"
    );
}

#[test]
fn another_workflow_or_no_run_excuses_nothing() {
    let mut b = base("failure", &failed("['.mason']"));
    b.runs[0].workflow = "other".into();
    assert_eq!(
        assess(&b, "develop", &batch(&failed("['.mason']"))),
        Verdict::Blame
    );
    b.runs.clear();
    assert_eq!(
        assess(&b, "develop", &batch(&failed("['.mason']"))),
        Verdict::Blame
    );
}
