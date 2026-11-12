// SPDX-License-Identifier: GPL-3.0-or-later

//! The dry run: walk scripted tasks through a graph with no model and no
//! network, through the real reducer, so a change can be seen before it is
//! applied. It routes with [`graph::after`], the function the daemon uses.

use crate::graph::{after, Outcome, Transition};
use crate::model::*;
use mason_core::*;
use mason_engine::{reduce, World};
use std::collections::{BTreeMap, VecDeque};
use toml_edit::DocumentMut;

#[derive(Clone, Debug, Default)]
pub struct Scenario {
    pub name: String,
    /// The filer declared the task explored.
    pub explored: bool,
    /// The clarity check passes.
    pub clear: bool,
    /// What each step reports, one entry per run of it; green once they run out.
    pub results: BTreeMap<String, VecDeque<Outcome>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum End {
    Landed,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraceStep {
    pub step: String,
    pub state: State,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Trace {
    pub route: Route,
    pub steps: Vec<TraceStep>,
    pub end: End,
}

impl Trace {
    pub fn names(&self) -> Vec<&str> {
        self.steps.iter().map(|s| s.step.as_str()).collect()
    }
}

const PROJECT: ProjectId = ProjectId(1);

/// The most steps a walk may take before it is called non-terminating.
pub const STEP_CAP: usize = 2000;

/// Walk one task. `choose(step, attempt)` says how each run of a step ends.
pub fn walk(
    def: &Definition,
    route: Route,
    choose: &mut dyn FnMut(&Step, u32) -> Outcome,
) -> Result<Trace, String> {
    let entry = def
        .entry(route.skipped_exploring())
        .ok_or("the definition has no entry step")?;
    let mut w = World::new();
    let mut at = 0u64;
    let mut apply = |w: &mut World, kind: EventKind| -> Result<(), String> {
        at += 1;
        let ev = Event {
            at: Timestamp(at),
            kind,
        };
        let (next, _) =
            reduce(w, &ev).map_err(|e| format!("the reducer refused {:?}: {e:?}", ev.kind))?;
        next.check_invariants()
            .map_err(|e| format!("invariant broken after {:?}: {e}", ev.kind))?;
        *w = next;
        Ok(())
    };
    apply(
        &mut w,
        EventKind::ProjectRegistered {
            project: PROJECT,
            name: "dry".into(),
        },
    )?;
    apply(
        &mut w,
        EventKind::TaskFiled {
            project: PROJECT,
            title: "dry run".into(),
            body: String::new(),
            priority: Priority::Normal,
            trust: TrustSource::Owner,
            explored: route.skipped_exploring(),
            proposed: false,
            blockers: vec![],
            not_before: None,
        },
    )?;
    let task = TaskId(0);
    apply(
        &mut w,
        EventKind::Admitted {
            task,
            entry: Some(Entry {
                def: def.hash.clone(),
                step: entry.name.clone(),
                state: entry.category,
                route,
            }),
        },
    )?;

    let mut trace = Trace {
        route,
        steps: vec![],
        end: End::Failed(String::new()),
    };
    let mut current = entry.name.clone();
    for _ in 0..STEP_CAP {
        let step = def
            .step(&current)
            .ok_or_else(|| format!("`{current}` is not a step"))?;
        let attempt = w.tasks[&task]
            .step_attempts
            .get(&current)
            .copied()
            .unwrap_or(0);
        let outcome = choose(step, attempt);
        trace.steps.push(TraceStep {
            step: current.clone(),
            state: step.category,
            outcome,
        });
        match after(def, step, outcome, attempt, None) {
            Transition::Next { step: n, state } => {
                apply(
                    &mut w,
                    EventKind::StepSucceeded {
                        task,
                        next: Some(Next {
                            step: n.clone(),
                            state,
                        }),
                    },
                )?;
                current = n;
            }
            Transition::Land => {
                apply(
                    &mut w,
                    EventKind::StepSucceeded {
                        task,
                        next: Some(Next {
                            step: String::new(),
                            state: State::Landed,
                        }),
                    },
                )?;
                trace.end = End::Landed;
                return Ok(trace);
            }
            Transition::SendBack { step: back } => {
                apply(
                    &mut w,
                    EventKind::SentBack {
                        task,
                        reason: "red".into(),
                        to_step: back.clone(),
                    },
                )?;
                current = back;
            }
            Transition::Fail(reason) => {
                apply(
                    &mut w,
                    EventKind::StepFailed {
                        task,
                        reason: reason.clone(),
                        class: None,
                    },
                )?;
                trace.end = End::Failed(reason);
                return Ok(trace);
            }
        }
    }
    Err(format!(
        "the walk took more than {STEP_CAP} steps and did not end"
    ))
}

/// Run a scripted scenario.
pub fn dry_run(def: &Definition, sc: &Scenario) -> Result<Trace, String> {
    if def.steps.iter().any(|s| s.next.len() > 1) {
        return Err(
            "cannot dry-run a definition with fan-out: parallel steps cannot run yet".into(),
        );
    }
    let route = if sc.explored && def.explored_entry.is_some() {
        Route::Flag
    } else if sc.clear && def.explored_entry.is_some() {
        Route::Clarity
    } else {
        Route::Explore
    };
    let mut queues = sc.results.clone();
    walk(def, route, &mut |s, _| {
        queues
            .get_mut(&s.name)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Outcome::Green)
    })
}

/// Scenarios worth running against any definition: the clean paths, and for
/// every step that can loop back, going red once and going red every time.
pub fn default_scenarios(def: &Definition) -> Vec<Scenario> {
    let mut v = vec![Scenario {
        name: "unflagged task, everything green".into(),
        ..Default::default()
    }];
    if def.explored_entry.is_some() {
        v.push(Scenario {
            name: "flagged explored, everything green".into(),
            explored: true,
            ..Default::default()
        });
        v.push(Scenario {
            name: "passes the clarity check, everything green".into(),
            clear: true,
            ..Default::default()
        });
    }
    for s in def.steps.iter().filter(|s| s.on_red.is_some()) {
        let rounds = s.max_rounds.unwrap_or(1) as usize;
        v.push(Scenario {
            name: format!("`{}` red once, then green", s.name),
            results: [(s.name.clone(), VecDeque::from([Outcome::Red]))].into(),
            ..Default::default()
        });
        v.push(Scenario {
            name: format!("`{}` red every time", s.name),
            results: [(
                s.name.clone(),
                VecDeque::from(vec![Outcome::Red; rounds + 1]),
            )]
            .into(),
            ..Default::default()
        });
    }
    if let Some(first) = def.step(&def.start) {
        v.push(Scenario {
            name: format!("`{}` fails", first.name),
            results: [(first.name.clone(), VecDeque::from([Outcome::Fail]))].into(),
            ..Default::default()
        });
    }
    v
}

/// Read scenarios from TOML:
///
/// ```toml
/// [[scenario]]
/// name = "red once"
/// explored = true
/// [scenario.results]
/// test = ["red", "green"]
/// ```
pub fn parse_scenarios(text: &str) -> Result<Vec<Scenario>, String> {
    let doc: DocumentMut = text
        .parse()
        .map_err(|e: toml_edit::TomlError| format!("not valid TOML: {e}"))?;
    let list = doc
        .get("scenario")
        .and_then(|s| s.as_array_of_tables())
        .ok_or("expected [[scenario]] tables")?;
    let mut out = vec![];
    for (i, t) in list.iter().enumerate() {
        let mut sc = Scenario {
            name: t
                .get("name")
                .and_then(|v| v.as_str())
                .map_or_else(|| format!("scenario {}", i + 1), String::from),
            explored: t.get("explored").and_then(|v| v.as_bool()).unwrap_or(false),
            clear: t.get("clear").and_then(|v| v.as_bool()).unwrap_or(false),
            ..Default::default()
        };
        if let Some(r) = t.get("results").and_then(|r| r.as_table_like()) {
            for (step, v) in r.iter() {
                let arr = v
                    .as_array()
                    .ok_or_else(|| format!("results.{step} must be a list"))?;
                let mut q = VecDeque::new();
                for x in arr.iter() {
                    let n = x.as_str().and_then(Outcome::from_name).ok_or_else(|| {
                        format!("results.{step}: use \"green\", \"red\" or \"fail\"")
                    })?;
                    q.push_back(n);
                }
                sc.results.insert(step.to_string(), q);
            }
        }
        out.push(sc);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::builtin;

    fn run(sc: Scenario) -> Trace {
        dry_run(&builtin(), &sc).unwrap()
    }

    #[test]
    fn an_unflagged_task_explores_and_a_flagged_one_skips_to_develop() {
        let t = run(Scenario::default());
        assert_eq!(
            (t.route, t.names(), &t.end),
            (
                Route::Explore,
                vec!["explore", "develop", "test", "land"],
                &End::Landed
            )
        );
        let t = run(Scenario {
            explored: true,
            ..Default::default()
        });
        assert_eq!(
            (t.route, t.names(), &t.end),
            (Route::Flag, vec!["develop", "test", "land"], &End::Landed)
        );
        let t = run(Scenario {
            clear: true,
            ..Default::default()
        });
        assert_eq!(t.route, Route::Clarity);
        assert_eq!(t.names()[0], "develop");
    }

    #[test]
    fn red_goes_back_and_is_bounded() {
        let red_once = Scenario {
            results: [("test".to_string(), VecDeque::from([Outcome::Red]))].into(),
            ..Default::default()
        };
        let t = run(red_once);
        assert_eq!(
            t.names(),
            vec!["explore", "develop", "test", "develop", "test", "land"]
        );
        let always = Scenario {
            results: [("test".to_string(), VecDeque::from(vec![Outcome::Red; 10]))].into(),
            ..Default::default()
        };
        let t = run(always);
        assert!(
            matches!(&t.end, End::Failed(m) if m.contains("still red after 3 rounds")),
            "{:?}",
            t.end
        );
        assert_eq!(t.steps.iter().filter(|s| s.step == "test").count(), 3);
    }

    #[test]
    fn a_failing_step_ends_the_task_failed() {
        let t = run(Scenario {
            results: [("explore".to_string(), VecDeque::from([Outcome::Fail]))].into(),
            ..Default::default()
        });
        assert!(matches!(t.end, End::Failed(_)));
        assert_eq!(t.names(), vec!["explore"]);
    }

    #[test]
    fn every_default_scenario_ends_for_the_builtin_graph() {
        let d = builtin();
        let scs = default_scenarios(&d);
        assert!(scs.len() >= 6);
        for sc in scs {
            let t = dry_run(&d, &sc).unwrap_or_else(|e| panic!("{}: {e}", sc.name));
            assert!(matches!(t.end, End::Landed | End::Failed(_)), "{}", sc.name);
        }
    }

    #[test]
    fn scenarios_can_be_read_from_toml() {
        let s = parse_scenarios("[[scenario]]\nname = \"x\"\nexplored = true\n[scenario.results]\ntest = [\"red\", \"green\"]\n[[scenario]]\n").unwrap();
        assert_eq!(s.len(), 2);
        assert!(
            s[0].explored
                && s[0].results["test"]
                    .iter()
                    .copied()
                    .eq([Outcome::Red, Outcome::Green])
        );
        assert_eq!(s[1].name, "scenario 2");
        assert!(
            parse_scenarios("[[scenario]]\n[scenario.results]\ntest = [\"purple\"]\n").is_err()
        );
        assert!(parse_scenarios("nothing = 1\n").is_err());
    }
}
