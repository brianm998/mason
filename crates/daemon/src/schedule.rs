// SPDX-License-Identifier: GPL-3.0-or-later

//! Recurring jobs (Part 14). `[schedule.<name>]` in a project's local overlay says what runs
//! and how often. A job is one of:
//!
//! - `task` (the default): files one Low-priority task, which runs through the project's own
//!   graph with the same resources, monitor and alerts as any task. Not filed again while the
//!   last one is still open.
//! - `base-health`, `mutants`, `live`: runs a command on a clean checkout of the base branch and
//!   reads the failures from its output. The first run only records the failures it finds as
//!   the baseline. Every later run compares with the baseline: only failures not in it file
//!   a Low-priority task, and the baseline becomes what failed this time. `mutants` is meant for
//!   `cargo mutants` (survivors are read from `mutants.out/missed.txt`) and `live` for the
//!   adapters' live runs; both need what they run, so they are only ever scheduled by the owner.
//! - `ci`: dispatches a slow CI workflow on the base branch when it matters, follows the run,
//!   reruns its failed jobs once and files tasks for the reds and the flakes (`ci_schedule`).
//!   Off unless configured.

use crate::config::ProjectCfg;
use crate::daemon::{now_ms, Daemon};
use mason_core::introspect::{new_failures, parse_every};
use mason_core::*;
use mason_store::ScheduleRow;
use std::sync::Arc;
use std::time::{Duration, Instant};
use toml_edit::DocumentMut;

/// A schedule as written in the overlay.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub name: String,
    pub kind: String,
    pub every_ms: u64,
    pub title: String,
    pub body: String,
    pub command: Option<String>,
    /// `tap` (the `not ok` lines) or `lines` (every line of the results file).
    pub parser: String,
    pub results_file: Option<String>,
    pub timeout: Duration,
    /// The `ci` kind's settings.
    pub ci: Option<CiSpec>,
}

/// What a `kind = "ci"` schedule says (s175).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiSpec {
    /// The workflow file, `gherkin.yml`.
    pub workflow: String,
    /// The branch to dispatch on; the project's base branch when unset.
    pub git_ref: Option<String>,
    pub inputs: Vec<(String, String)>,
    /// The run-name the workflow gives a dispatch, which is how its runs are told from others.
    pub run_name: String,
    /// Minutes after midnight, local time: once a day at this time (instead of `every`).
    pub at: Option<u32>,
    /// Inline globs, or the workflow file whose `on.pull_request.paths` holds them.
    pub paths: Vec<String>,
    pub paths_from: Option<String>,
    pub group_by: mason_core::ci_schedule::GroupBy,
    pub max_tickets: usize,
}

impl Default for CiSpec {
    fn default() -> CiSpec {
        CiSpec {
            workflow: String::new(),
            git_ref: None,
            inputs: vec![],
            run_name: String::new(),
            at: None,
            paths: vec![],
            paths_from: None,
            group_by: Default::default(),
            max_tickets: 6,
        }
    }
}

pub const KINDS: [&str; 5] = ["task", "base-health", "mutants", "live", "ci"];

pub fn parse(id: &str, doc: &DocumentMut) -> Result<Vec<Spec>, String> {
    let Some(t) = doc.get("schedule").and_then(|v| v.as_table_like()) else {
        return Ok(vec![]);
    };
    let mut out = vec![];
    for (name, item) in t.iter() {
        let at = format!("{id}: `[schedule.{name}]`");
        let tbl = item
            .as_table_like()
            .ok_or_else(|| format!("{at} must be a table"))?;
        let mut spec = Spec {
            name: name.to_string(),
            kind: "task".into(),
            every_ms: 0,
            title: String::new(),
            body: String::new(),
            command: None,
            parser: "tap".into(),
            results_file: None,
            timeout: Duration::from_secs(1800),
            ci: None,
        };
        let mut ci = CiSpec::default();
        let mut ci_keys: Vec<&str> = vec![];
        for (k, v) in tbl.iter() {
            let s = || {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("{at} `{k}` must be a string"))
            };
            match k {
                "kind" => {
                    spec.kind = s()?;
                    if !KINDS.contains(&spec.kind.as_str()) {
                        return Err(format!(
                            "{at} `kind` must be one of {}, not {:?}",
                            KINDS.join(", "),
                            spec.kind
                        ));
                    }
                }
                "every" => {
                    spec.every_ms = parse_every(&s()?).ok_or_else(|| {
                        format!(
                        "{at} `every` must be hourly, daily, weekly or a number with s, m, h or d"
                    )
                    })?
                }
                "title" => spec.title = s()?,
                "body" => spec.body = s()?,
                "command" => spec.command = Some(s()?).filter(|c| !c.trim().is_empty()),
                "parser" => {
                    spec.parser = s()?;
                    if !["tap", "lines"].contains(&spec.parser.as_str()) {
                        return Err(format!("{at} `parser` must be \"tap\" or \"lines\""));
                    }
                }
                "results_file" => spec.results_file = Some(s()?).filter(|c| !c.is_empty()),
                "timeout_s" => {
                    spec.timeout =
                        Duration::from_secs(v.as_integer().filter(|n| *n >= 1).ok_or_else(|| {
                            format!("{at} `timeout_s` must be a whole number from 1")
                        })? as u64)
                }
                "workflow" | "ref" | "run_name" | "at" | "paths_from" | "group_by" => {
                    ci_keys.push(k);
                    let t = s()?;
                    match k {
                        "workflow" => ci.workflow = t,
                        "ref" => ci.git_ref = Some(t).filter(|r| !r.is_empty()),
                        "run_name" => ci.run_name = t,
                        "paths_from" => ci.paths_from = Some(t).filter(|r| !r.is_empty()),
                        "at" => {
                            ci.at =
                                Some(mason_core::ci_schedule::parse_at(&t).ok_or_else(|| {
                                    format!("{at} `at` must be a time of day like \"23:00\"")
                                })?)
                        }
                        _ => {
                            ci.group_by = mason_core::ci_schedule::GroupBy::parse(&t)
                                .map_err(|e| format!("{at} {e}"))?
                        }
                    }
                }
                "inputs" => {
                    ci_keys.push(k);
                    let tb = v
                        .as_table_like()
                        .ok_or_else(|| format!("{at} `inputs` must be a table of strings"))?;
                    for (ik, iv) in tb.iter() {
                        let val = iv
                            .as_str()
                            .ok_or_else(|| format!("{at} `inputs.{ik}` must be a string"))?;
                        ci.inputs.push((ik.to_string(), val.to_string()));
                    }
                }
                "paths" => {
                    ci_keys.push(k);
                    let arr = v
                        .as_array()
                        .ok_or_else(|| format!("{at} `paths` must be a list of globs"))?;
                    for g in arr.iter() {
                        ci.paths.push(
                            g.as_str()
                                .filter(|g| !g.is_empty())
                                .ok_or_else(|| format!("{at} `paths` holds only globs"))?
                                .to_string(),
                        );
                    }
                }
                "max_tickets" => {
                    ci_keys.push(k);
                    ci.max_tickets = v
                        .as_integer()
                        .filter(|n| *n >= 0)
                        .ok_or_else(|| format!("{at} `max_tickets` must be a whole number"))?
                        as usize;
                }
                other => return Err(format!("{at} has an unknown key `{other}`")),
            }
        }
        if spec.kind == "ci" {
            if ci.workflow.trim().is_empty() {
                return Err(format!("{at} is a ci schedule and needs a `workflow` (the file, such as \"gherkin.yml\")"));
            }
            if ci.at.is_some() && spec.every_ms != 0 {
                return Err(format!("{at} takes `at` or `every`, not both"));
            }
            if ci.at.is_none() && spec.every_ms == 0 {
                return Err(format!("{at} needs `at = \"23:00\"` or an `every`"));
            }
            if ci.paths_from.is_some() && !ci.paths.is_empty() {
                return Err(format!("{at} takes `paths` or `paths_from`, not both"));
            }
            if ci.at.is_some() {
                spec.every_ms = 24 * 3_600_000;
            }
            spec.ci = Some(ci);
        } else if let Some(k) = ci_keys.first() {
            return Err(format!("{at} `{k}` belongs to `kind = \"ci\"`"));
        }
        if spec.every_ms == 0 {
            return Err(format!(
                "{at} needs `every = \"daily\"` (or hourly, weekly, 6h, 30m)"
            ));
        }
        if spec.kind == "task" && spec.title.trim().is_empty() {
            return Err(format!("{at} is a task schedule and needs a `title`"));
        }
        // Defaults that make `mutants` do what it is for.
        if spec.kind == "mutants" {
            spec.command
                .get_or_insert_with(|| "cargo mutants --no-shuffle".into());
            spec.results_file
                .get_or_insert_with(|| "mutants.out/missed.txt".into());
            spec.parser = "lines".into();
        }
        if spec.kind == "live" && spec.command.is_none() {
            return Err(format!("{at} is a live run and needs a `command`"));
        }
        out.push(spec);
    }
    Ok(out)
}

impl ProjectCfg {
    /// The project's recurring jobs (validated when the project loaded).
    pub fn schedules(&self) -> Vec<Spec> {
        self.overlay_text
            .parse::<DocumentMut>()
            .ok()
            .and_then(|d| parse(&self.id, &d).ok())
            .unwrap_or_default()
    }
}

/// The row name of a project's schedule.
pub fn row_name(project: &str, name: &str) -> String {
    format!("{project}/{name}")
}

/// Run one schedule now. Returns what happened, in a sentence, and records it.
pub fn run(d: &Arc<Daemon>, project: &ProjectCfg, spec: &Spec) -> String {
    let key = row_name(&project.id, &spec.name);
    let now = now_ms();
    let mut row = d
        .store
        .schedule(&key)
        .ok()
        .flatten()
        .unwrap_or_else(|| ScheduleRow {
            name: key.clone(),
            project: project.id.clone(),
            kind: spec.kind.clone(),
            every_ms: spec.every_ms,
            ..Default::default()
        });
    row.kind = spec.kind.clone();
    row.every_ms = spec.every_ms;
    let before = (row.last_run, row.runs);
    row.last_run = now;
    row.runs += 1;
    let say = if spec.kind == "task" {
        run_task(d, project, spec, &mut row)
    } else if spec.kind == "ci" {
        let (say, decided) = crate::ci_schedule::run(d, project, spec, &mut row, now);
        if !decided {
            // Only a followed run moved: the schedule did not run again.
            row.last_run = before.0;
            row.runs = before.1;
        }
        say
    } else {
        run_check(d, project, spec, &mut row)
    };
    row.last_status = say.clone();
    let _ = d.store.put_schedule(&row);
    d.poke();
    tracing::info!(schedule = %key, %say, "recurring job ran");
    say
}

fn is_live(d: &Daemon, task: u64) -> bool {
    d.world()
        .ok()
        .and_then(|w| w.tasks.get(&TaskId(task)).map(|t| t.state))
        .is_some_and(|s| !matches!(s, State::Landed | State::Canceled | State::Failed))
}

/// The open task of the project whose title is `title` (as `file_low` cuts it), if any.
fn open_task_titled(d: &Daemon, project: &ProjectCfg, title: &str) -> Option<TaskId> {
    let pid = d.project_id_by_name(&project.id)?;
    let w = d.world().ok()?;
    same_open_title(
        w.tasks
            .values()
            .filter(|t| t.project == pid && !t.state.is_terminal())
            .map(|t| (t.id, t.title.as_str())),
        title,
    )
}

/// The id among `open` (id, title) whose title is `title` cut to the length a filed task keeps.
pub fn same_open_title<'a>(
    open: impl Iterator<Item = (TaskId, &'a str)>,
    title: &str,
) -> Option<TaskId> {
    let cut: String = title.chars().take(120).collect();
    open.filter(|(_, t)| *t == cut)
        .map(|(id, _)| id)
        .min_by_key(|id| id.0)
}

/// File a Low-priority task, unless the last one is still open. Returns its id.
pub fn file_low(
    d: &Arc<Daemon>,
    project: &ProjectCfg,
    title: &str,
    body: &str,
) -> Result<TaskId, String> {
    let pid = d.project_id_by_name(&project.id).ok_or("no such project")?;
    let appended = d
        .emit(EventKind::TaskFiled {
            project: pid,
            title: title.chars().take(120).collect(),
            body: body.to_string(),
            priority: Priority::Low,
            trust: TrustSource::OwnerAgent,
            explored: true,
            proposed: false,
            blockers: vec![],
            not_before: None,
        })
        .map_err(|e| e.to_string())?;
    appended
        .effects
        .iter()
        .find_map(|e| match e {
            Effect::TaskCreated { task } => Some(*task),
            _ => None,
        })
        .ok_or_else(|| "the task was not created".into())
}

fn run_task(d: &Arc<Daemon>, project: &ProjectCfg, spec: &Spec, row: &mut ScheduleRow) -> String {
    if let Some(prev) = row.last_task.filter(|t| is_live(d, *t)) {
        return format!("not filed: task #{prev} from the last run is still open");
    }
    let body = if spec.body.trim().is_empty() {
        format!("Filed by the schedule `{}` ({}).", spec.name, project.id)
    } else {
        spec.body.clone()
    };
    match file_low(d, project, &spec.title, &body) {
        Ok(t) => {
            row.last_task = Some(t.0);
            format!("filed task #{} at low priority", t.0)
        }
        Err(e) => format!("could not file: {e}"),
    }
}

/// The names of what failed in a command's output.
pub fn failures(parser: &str, output: &str, results: Option<&str>, status_ok: bool) -> Vec<String> {
    let mut names: Vec<String> = match parser {
        "lines" => results
            .unwrap_or(output)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(String::from)
            .collect(),
        _ => output
            .lines()
            .filter_map(|l| l.trim_start().strip_prefix("not ok"))
            .map(|rest| {
                rest.trim_start_matches(|c: char| c.is_ascii_digit() || c.is_whitespace())
                    .trim_start_matches('-')
                    .trim()
                    .to_string()
            })
            .filter(|n| !n.is_empty())
            .collect(),
    };
    if names.is_empty() && !status_ok {
        // It failed and said nothing we can name: the run as a whole is the failure.
        names.push("(the command failed without naming a test)".into());
    }
    names.sort();
    names.dedup();
    names
}

fn run_check(d: &Arc<Daemon>, project: &ProjectCfg, spec: &Spec, row: &mut ScheduleRow) -> String {
    let cmd = spec
        .command
        .clone()
        .unwrap_or_else(|| project.test_cmd.clone());
    let wt = d
        .cfg
        .worktrees_dir()
        .join("schedules")
        .join(format!("{}-{}", project.id, spec.name));
    mason_landing::git::remove_worktree(&project.path, &wt);
    if let Err(e) = mason_landing::git::run(
        &project.path,
        &[
            "worktree",
            "add",
            "--detach",
            &wt.to_string_lossy(),
            &project.base_ref(),
        ],
    ) {
        return format!("could not check out {}: {e}", project.base_ref());
    }
    let ran = run_in(&wt, &cmd, spec.timeout);
    let results = spec
        .results_file
        .as_ref()
        .and_then(|f| std::fs::read_to_string(wt.join(f)).ok());
    mason_landing::git::remove_worktree(&project.path, &wt);
    let (ok, output) = match ran {
        Ok(x) => x,
        Err(e) => return format!("could not run the command: {e}"),
    };
    let current = failures(&spec.parser, &output, results.as_deref(), ok);
    let Some(baseline) = row.baseline.clone() else {
        row.baseline = Some(current.clone());
        return format!(
            "baseline recorded: {} known failure(s); nothing filed",
            current.len()
        );
    };
    let fresh = new_failures(&current, &baseline);
    row.baseline = Some(current.clone());
    let mut filed = vec![];
    for name in fresh.iter().take(10) {
        let title = format!("{} is red on {}: {name}", spec.name, project.base_ref());
        let body = format!(
            "The scheduled run `{}` ran `{cmd}` on `{}` and `{name}` failed. It was not failing in the previous run, so this is new.\n\nThe end of the output:\n```\n{}\n```",
            spec.name,
            project.base_ref(),
            tail(&output, 30)
        );
        // One task per failure: an open task already carrying this title is that failure's.
        if let Some(open) = open_task_titled(d, project, &title) {
            tracing::info!(task = open.0, %title, "not filed: a task for this failure is open");
            continue;
        }
        match file_low(d, project, &title, &body) {
            Ok(t) => filed.push(t.0),
            Err(e) => tracing::warn!(%e, "could not file the task for a new failure"),
        }
    }
    if let Some(t) = filed.last() {
        row.last_task = Some(*t);
    }
    format!(
        "{} failing now, {} already in the baseline, {} new; filed {}",
        current.len(),
        current.len() - fresh.len(),
        fresh.len(),
        if filed.is_empty() {
            "nothing".to_string()
        } else {
            filed
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(", ")
        }
    )
}

fn tail(s: &str, lines: usize) -> String {
    let all: Vec<&str> = s.lines().collect();
    all[all.len().saturating_sub(lines)..].join("\n")
}

/// Run `sh -c cmd` in `dir`, killing it after `limit`. `(exited 0, stdout and stderr)`.
fn run_in(dir: &std::path::Path, cmd: &str, limit: Duration) -> Result<(bool, String), String> {
    let out = dir.join(".mason-schedule-output.txt");
    let file = std::fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(file.try_clone().map_err(|e| e.to_string())?)
        .stderr(file)
        .spawn()
        .map_err(|e| e.to_string())?;
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => return Err(e.to_string()),
        }
    };
    let text = crate::jobs::read_tail_lossy(&out).unwrap_or_default();
    let _ = std::fs::remove_file(&out);
    match status {
        Some(s) => Ok((s.success(), text)),
        None => Err(format!("timed out after {}s", limit.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedules_are_read_and_checked() {
        let doc: DocumentMut = r#"
[schedule.nightly]
kind = "base-health"
every = "daily"
[schedule.weekly-note]
every = "weekly"
title = "Look at the logs"
[schedule.mutants]
kind = "mutants"
every = "7d"
"#
        .parse()
        .unwrap();
        let s = parse("web", &doc).unwrap();
        assert_eq!(s.len(), 3);
        let m = s.iter().find(|s| s.name == "mutants").unwrap();
        assert_eq!(m.results_file.as_deref(), Some("mutants.out/missed.txt"));
        assert!(m.command.as_deref().unwrap().contains("cargo mutants"));
        for bad in [
            "[schedule.x]\nkind = \"nope\"\nevery = \"daily\"\n",
            "[schedule.x]\nkind = \"base-health\"\n",
            "[schedule.x]\nevery = \"daily\"\n",
            "[schedule.x]\nkind = \"live\"\nevery = \"daily\"\n",
            "[schedule.x]\nkind = \"base-health\"\nevery = \"daily\"\nfast = true\n",
            "[schedule.x]\nkind = \"base-health\"\nevery = \"daily\"\nworkflow = \"a.yml\"\n",
            "[schedule.x]\nkind = \"ci\"\nat = \"23:00\"\n",
            "[schedule.x]\nkind = \"ci\"\nworkflow = \"a.yml\"\n",
            "[schedule.x]\nkind = \"ci\"\nworkflow = \"a.yml\"\nat = \"25:00\"\n",
            "[schedule.x]\nkind = \"ci\"\nworkflow = \"a.yml\"\nat = \"23:00\"\nevery = \"daily\"\n",
            "[schedule.x]\nkind = \"ci\"\nworkflow = \"a.yml\"\nat = \"23:00\"\npaths = [\"a\"]\npaths_from = \"b.yml\"\n",
            "[schedule.x]\nkind = \"ci\"\nworkflow = \"a.yml\"\nat = \"23:00\"\ngroup_by = \"feature\"\n",
            "[schedule.x]\nkind = \"ci\"\nworkflow = \"a.yml\"\nat = \"23:00\"\nfast = true\n",
            "[schedule.x]\nkind = \"ci\"\nworkflow = \"a.yml\"\nat = \"23:00\"\ninputs = { n = 3 }\n",
        ] {
            assert!(parse("web", &bad.parse().unwrap()).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_ci_schedule_reads_all_its_keys() {
        let doc: DocumentMut = r#"
[schedule.nightly]
kind = "ci"
workflow = "gherkin.yml"
inputs = { features = "" }
run_name = "Gherkin: full"
at = "23:00"
paths_from = ".github/workflows/gherkin-smoke.yml"
group_by = "<group>/<leg>"
max_tickets = 4
[schedule.inline]
kind = "ci"
workflow = "slow.yml"
every = "12h"
paths = ["src/**", "Cargo.toml"]
"#
        .parse()
        .unwrap();
        let s = parse("star", &doc).unwrap();
        let n = s.iter().find(|s| s.name == "nightly").unwrap();
        let c = n.ci.as_ref().unwrap();
        assert_eq!(n.every_ms, 24 * 3_600_000);
        assert_eq!(c.at, Some(23 * 60));
        assert_eq!(c.inputs, vec![("features".to_string(), String::new())]);
        assert_eq!(c.max_tickets, 4);
        assert!(c.paths_from.is_some() && c.paths.is_empty());
        let i = s.iter().find(|s| s.name == "inline").unwrap();
        assert_eq!(i.every_ms, 12 * 3_600_000);
        let c = i.ci.as_ref().unwrap();
        assert_eq!((c.paths.len(), c.max_tickets, c.at), (2, 6, None));
        assert!(s.iter().all(|s| s.kind == "ci"));
    }

    #[test]
    fn a_failure_with_an_open_task_is_not_filed_again() {
        let open = [
            (
                TaskId(7),
                "exit-scripts is red on origin/develop: FAILED: scripts/exit/s166.sh",
            ),
            (TaskId(9), "something else"),
        ];
        assert_eq!(
            same_open_title(
                open.iter().copied(),
                "exit-scripts is red on origin/develop: FAILED: scripts/exit/s166.sh"
            ),
            Some(TaskId(7))
        );
        assert_eq!(
            same_open_title(
                open.iter().copied(),
                "exit-scripts is red on origin/develop: FAILED: scripts/exit/s12.sh"
            ),
            None
        );
        // A long title is compared as it is filed, cut to 120 characters.
        let long = format!("x{}", "y".repeat(200));
        let cut: String = long.chars().take(120).collect();
        assert_eq!(
            same_open_title([(TaskId(1), cut.as_str())].into_iter(), &long),
            Some(TaskId(1))
        );
    }

    #[test]
    fn failures_are_named_from_the_output() {
        let tap = "1..3\nok 1 - a\nnot ok 2 - b::c\nnot ok 3 d\n";
        assert_eq!(failures("tap", tap, None, false), vec!["b::c", "d"]);
        assert_eq!(
            failures("tap", "boom\n", None, false),
            vec!["(the command failed without naming a test)"]
        );
        assert!(failures("tap", "ok 1 - a\n", None, true).is_empty());
        assert_eq!(
            failures(
                "lines",
                "ignored",
                Some("src/a.rs:1: x\n\nsrc/b.rs:2: y\n"),
                false
            ),
            vec!["src/a.rs:1: x", "src/b.rs:2: y"]
        );
    }
}
