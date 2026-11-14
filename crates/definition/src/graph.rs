// SPDX-License-Identifier: GPL-3.0-or-later

//! Where a task goes after a step. This one function is used by the daemon to
//! choose its next event and by the dry run to walk scripted tasks, so what you
//! simulate is what runs.

use crate::model::*;
use mason_core::State;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Green,
    Red,
    Fail,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Green => "green",
            Outcome::Red => "red",
            Outcome::Fail => "fail",
        }
    }

    pub fn from_name(s: &str) -> Option<Outcome> {
        [Outcome::Green, Outcome::Red, Outcome::Fail]
            .into_iter()
            .find(|o| o.name() == s)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transition {
    /// Start this step, whose category is `state`.
    Next {
        step: String,
        state: State,
    },
    /// Landing is done: the task is merged.
    Land,
    /// Back to this developing step.
    SendBack {
        step: String,
    },
    Fail(String),
}

/// What follows `step` given how it ended. `attempt` is how many times this
/// step has run for the task; `rounds` is its effective `max_rounds` after the
/// configuration layers (the definition's own value if `None`).
pub fn after(
    def: &Definition,
    step: &Step,
    outcome: Outcome,
    attempt: u32,
    rounds: Option<u32>,
) -> Transition {
    match outcome {
        Outcome::Fail => Transition::Fail(format!("step `{}` failed", step.name)),
        Outcome::Green => {
            if step.next.is_empty() {
                return if step.kind == StepType::Landing {
                    Transition::Land
                } else {
                    Transition::Fail(format!("step `{}` has no next step", step.name))
                };
            }
            if step.next.len() > 1 {
                return Transition::Fail(format!(
                    "step `{}` fans out, which cannot run yet",
                    step.name
                ));
            }
            match def.step(&step.next[0]) {
                Some(n) => Transition::Next {
                    step: n.name.clone(),
                    state: n.category,
                },
                None => Transition::Fail(format!("`{}` is not a step", step.next[0])),
            }
        }
        Outcome::Red => match &step.on_red {
            None => Transition::Fail(format!("step `{}` was red and has no on_red", step.name)),
            Some(back) => {
                let limit = rounds.or(step.max_rounds).unwrap_or(1);
                if attempt >= limit {
                    Transition::Fail(format!(
                        "step `{}` still red after {limit} round{}",
                        step.name,
                        if limit == 1 { "" } else { "s" }
                    ))
                } else {
                    Transition::SendBack { step: back.clone() }
                }
            }
        },
    }
}

/// Steps still to run from `from`, counting `from` itself: the shortest path
/// through `next` to a step that ends the task, ignoring `on_red` loops. A
/// fan-out counts its shortest branch. This is what makes tasks comparable
/// across projects with different graphs (Part 7). An unknown step, or one
/// that cannot reach the end, counts as the whole graph.
pub fn steps_remaining(def: &Definition, from: &str) -> u32 {
    let mut best: std::collections::BTreeMap<&str, u32> = std::collections::BTreeMap::new();
    let mut frontier = vec![(from, 1u32)];
    let mut shortest: Option<u32> = None;
    while let Some((name, depth)) = frontier.pop() {
        if best.get(name).is_some_and(|d| *d <= depth) {
            continue;
        }
        best.insert(name, depth);
        let Some(step) = def.step(name) else { continue };
        if step.next.is_empty() {
            shortest = Some(shortest.map_or(depth, |s| s.min(depth)));
            continue;
        }
        for n in &step.next {
            frontier.push((n.as_str(), depth + 1));
        }
    }
    shortest.unwrap_or(def.steps.len() as u32)
}

/// The steps on the shortest start-to-landed path of a definition: what a
/// task's [`steps_remaining`] is a share of when tasks of different projects
/// are compared (never less than 1).
pub fn route_length(def: &Definition) -> u32 {
    steps_remaining(def, &def.start).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtin::builtin;

    #[test]
    fn green_goes_forward_and_landing_lands() {
        let d = builtin();
        let t = |name: &str, o, attempt| after(&d, d.step(name).unwrap(), o, attempt, None);
        assert_eq!(
            t("explore", Outcome::Green, 1),
            Transition::Next {
                step: "develop".into(),
                state: State::Developing
            }
        );
        assert_eq!(
            t("test", Outcome::Green, 1),
            Transition::Next {
                step: "land".into(),
                state: State::Landing
            }
        );
        assert_eq!(t("land", Outcome::Green, 1), Transition::Land);
    }

    #[test]
    fn red_loops_back_until_the_bound_and_then_fails() {
        let d = builtin();
        let test = d.step("test").unwrap();
        assert_eq!(
            after(&d, test, Outcome::Red, 1, None),
            Transition::SendBack {
                step: "develop".into()
            }
        );
        assert_eq!(
            after(&d, test, Outcome::Red, 2, None),
            Transition::SendBack {
                step: "develop".into()
            }
        );
        assert!(
            matches!(after(&d, test, Outcome::Red, 3, None), Transition::Fail(m) if m.contains("still red after 3 rounds"))
        );
        // A layer can change the bound.
        assert!(matches!(
            after(&d, test, Outcome::Red, 2, Some(2)),
            Transition::Fail(_)
        ));
        assert_eq!(
            after(&d, test, Outcome::Red, 4, Some(9)),
            Transition::SendBack {
                step: "develop".into()
            }
        );
    }

    #[test]
    fn red_without_on_red_and_fail_end_the_task() {
        let d = builtin();
        assert!(matches!(
            after(&d, d.step("develop").unwrap(), Outcome::Red, 1, None),
            Transition::Fail(_)
        ));
        assert!(matches!(
            after(&d, d.step("land").unwrap(), Outcome::Fail, 1, None),
            Transition::Fail(_)
        ));
    }

    #[test]
    fn steps_remaining_counts_the_shortest_path_ignoring_loops() {
        let d = builtin();
        // explore -> develop -> test -> land
        assert_eq!(steps_remaining(&d, "explore"), 4);
        assert_eq!(steps_remaining(&d, "develop"), 3);
        assert_eq!(steps_remaining(&d, "test"), 2);
        assert_eq!(steps_remaining(&d, "land"), 1);
        // `test` loops back to `develop` on red; that does not lengthen anything.
        assert_eq!(steps_remaining(&d, "develop"), 3);
        // Unknown steps count as the whole graph.
        assert_eq!(steps_remaining(&d, "nope"), 4);
    }
}
