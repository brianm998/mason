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
}
