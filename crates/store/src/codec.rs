// SPDX-License-Identifier: GPL-3.0-or-later

//! Text encoding of events for the log. Hand-written so `mason-core` stays
//! dependency-free. Fields are tab-separated; strings escape `\`, tab, CR, LF.

use mason_core::*;
use std::str::Split;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

fn err<T>(m: impl Into<String>) -> Result<T, CodecError> {
    Err(CodecError(m.into()))
}

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            c => o.push(c),
        }
    }
    o
}

fn unesc(s: &str) -> Result<String, CodecError> {
    let mut o = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            o.push(c);
            continue;
        }
        match it.next() {
            Some('\\') => o.push('\\'),
            Some('t') => o.push('\t'),
            Some('n') => o.push('\n'),
            Some('r') => o.push('\r'),
            other => return err(format!("bad escape {other:?}")),
        }
    }
    Ok(o)
}

#[derive(Default)]
struct W(Vec<String>);

impl W {
    fn s(&mut self, v: &str) -> &mut Self {
        self.0.push(esc(v));
        self
    }
    fn n(&mut self, v: u64) -> &mut Self {
        self.0.push(v.to_string());
        self
    }
    fn b(&mut self, v: bool) -> &mut Self {
        self.0.push(if v { "1" } else { "0" }.into());
        self
    }
    fn word(&mut self, v: &str) -> &mut Self {
        self.0.push(v.into());
        self
    }
    fn ts(&mut self, v: Option<Timestamp>) -> &mut Self {
        self.0.push(v.map_or("-".into(), |t| t.0.to_string()));
        self
    }
    fn ids(&mut self, v: &[TaskId]) -> &mut Self {
        self.0.push(
            v.iter()
                .map(|t| t.0.to_string())
                .collect::<Vec<_>>()
                .join(","),
        );
        self
    }
    fn out(&self) -> String {
        self.0.join("\t")
    }
}

struct R<'a>(Split<'a, char>);

impl R<'_> {
    fn raw(&mut self) -> Result<&str, CodecError> {
        self.0
            .next()
            .ok_or_else(|| CodecError("missing field".into()))
    }
    fn s(&mut self) -> Result<String, CodecError> {
        unesc(self.raw()?)
    }
    fn n(&mut self) -> Result<u64, CodecError> {
        self.raw()?
            .parse()
            .map_err(|_| CodecError("bad number".into()))
    }
    fn b(&mut self) -> Result<bool, CodecError> {
        match self.raw()? {
            "1" => Ok(true),
            "0" => Ok(false),
            o => err(format!("bad bool {o:?}")),
        }
    }
    fn ts(&mut self) -> Result<Option<Timestamp>, CodecError> {
        match self.raw()? {
            "-" => Ok(None),
            n => n
                .parse()
                .map(|v| Some(Timestamp(v)))
                .map_err(|_| CodecError("bad ts".into())),
        }
    }
    fn ids(&mut self) -> Result<Vec<TaskId>, CodecError> {
        let r = self.raw()?;
        if r.is_empty() {
            return Ok(vec![]);
        }
        r.split(',')
            .map(|x| {
                x.parse()
                    .map(TaskId)
                    .map_err(|_| CodecError("bad id".into()))
            })
            .collect()
    }
    /// Is there another field? (Newer fields trail older payloads.)
    fn more(&self) -> bool {
        self.0.clone().next().is_some()
    }
    /// A trailing string field that older payloads don't have.
    fn opt_s(&mut self) -> Result<String, CodecError> {
        match self.0.next() {
            Some(x) => unesc(x),
            None => Ok(String::new()),
        }
    }
    fn done(&mut self) -> Result<(), CodecError> {
        match self.0.next() {
            None => Ok(()),
            Some(_) => err("trailing fields"),
        }
    }
}

pub fn state_name(s: State) -> &'static str {
    match s {
        State::Proposed => "proposed",
        State::Queued => "queued",
        State::Exploring => "exploring",
        State::Developing => "developing",
        State::Testing => "testing",
        State::Review => "review",
        State::Landing => "landing",
        State::NeedsAttention => "needs_attention",
        State::Failed => "failed",
        State::Canceled => "canceled",
        State::Landed => "landed",
    }
}

fn state_from(s: &str) -> Result<State, CodecError> {
    State::ALL
        .into_iter()
        .find(|x| state_name(*x) == s)
        .ok_or_else(|| CodecError(format!("bad state {s:?}")))
}

pub(crate) fn priority_name(p: Priority) -> &'static str {
    match p {
        Priority::Urgent => "urgent",
        Priority::High => "high",
        Priority::Normal => "normal",
        Priority::Low => "low",
    }
}

fn priority_from(s: &str) -> Result<Priority, CodecError> {
    [
        Priority::Urgent,
        Priority::High,
        Priority::Normal,
        Priority::Low,
    ]
    .into_iter()
    .find(|p| priority_name(*p) == s)
    .ok_or_else(|| CodecError(format!("bad priority {s:?}")))
}

fn trust_name(t: TrustSource) -> &'static str {
    match t {
        TrustSource::Owner => "owner",
        TrustSource::OwnerAgent => "owner_agent",
        TrustSource::Outside => "outside",
    }
}

fn trust_from(s: &str) -> Result<TrustSource, CodecError> {
    [
        TrustSource::Owner,
        TrustSource::OwnerAgent,
        TrustSource::Outside,
    ]
    .into_iter()
    .find(|t| trust_name(*t) == s)
    .ok_or_else(|| CodecError(format!("bad trust {s:?}")))
}

/// Returns `(kind, payload)`. The timestamp is stored in its own column.
pub fn encode(k: &EventKind) -> (&'static str, String) {
    use EventKind::*;
    let mut w = W::default();
    let kind = match k {
        ProjectRegistered { project, name } => {
            w.n(project.0).s(name);
            "project_registered"
        }
        TaskFiled {
            project,
            title,
            body,
            priority,
            trust,
            explored,
            proposed,
            blockers,
            not_before,
        } => {
            w.n(project.0)
                .s(title)
                .word(priority_name(*priority))
                .word(trust_name(*trust))
                .b(*explored)
                .b(*proposed)
                .ids(blockers)
                .ts(*not_before)
                .s(body); // last, so logs written before bodies existed still decode
            "task_filed"
        }
        Approved { task } => {
            w.n(task.0);
            "approved"
        }
        Rejected { task } => {
            w.n(task.0);
            "rejected"
        }
        Admitted { task, entry } => {
            w.n(task.0);
            if let Some(e) = entry {
                w.s(&e.def)
                    .s(&e.step)
                    .word(state_name(e.state))
                    .word(e.route.name());
            }
            "admitted"
        }
        StepSucceeded { task, next } => {
            w.n(task.0);
            if let Some(n) = next {
                w.s(&n.step).word(state_name(n.state));
            }
            "step_succeeded"
        }
        SentBack {
            task,
            reason,
            to_step,
        } => {
            w.n(task.0).s(reason).s(to_step);
            "sent_back"
        }
        ExplorationRequested {
            task,
            reason,
            to_step,
        } => {
            w.n(task.0).s(reason).s(to_step);
            "exploration_requested"
        }
        ExploreAnyway { task, step } => {
            w.n(task.0).s(step);
            "explore_anyway"
        }
        QuestionAsked { task, question } => {
            w.n(task.0).s(question);
            "question_asked"
        }
        Answered {
            task,
            decision,
            answer,
            step,
        } => {
            w.n(task.0).n(decision.0).s(answer).s(step);
            "answered"
        }
        StepFailed {
            task,
            reason,
            class,
        } => {
            w.n(task.0).s(reason);
            if let Some(c) = class {
                w.word(c.name());
            }
            "step_failed"
        }
        Retried {
            task,
            to,
            step,
            auto,
        } => {
            w.n(task.0).word(state_name(*to)).s(step);
            if *auto {
                w.word("auto");
            }
            "retried"
        }
        Canceled {
            task,
            delete_branch,
        } => {
            w.n(task.0).b(*delete_branch);
            "canceled"
        }
        ClosedNoChange { task, reason } => {
            w.n(task.0).s(reason);
            "closed_no_change"
        }
        Paused { task } => {
            w.n(task.0);
            "paused"
        }
        Resumed { task } => {
            w.n(task.0);
            "resumed"
        }
        WaitStarted { task, on, class } => {
            w.n(task.0).s(on).s(class.as_deref().unwrap_or(""));
            "wait_started"
        }
        WaitEnded { task } => {
            w.n(task.0);
            "wait_ended"
        }
        BlockerAdded { task, blocker } => {
            w.n(task.0).n(blocker.0);
            "blocker_added"
        }
        BlockerRemoved { task, blocker } => {
            w.n(task.0).n(blocker.0);
            "blocker_removed"
        }
        BlockersChanged {
            task,
            released,
            added,
        } => {
            w.n(task.0).ids(released).ids(added);
            "blockers_changed"
        }
        Repinned {
            task,
            from,
            to,
            changed,
        } => {
            w.n(task.0).s(from).s(to).s(changed);
            "repinned"
        }
        Revived { task } => {
            w.n(task.0);
            "revived"
        }
        TrackerEdited {
            task,
            title,
            body,
            priority,
        } => {
            w.n(task.0).s(title).s(body).s(priority_name(*priority));
            "tracker_edited"
        }
        Tick => "tick",
    };
    (kind, w.out())
}

pub fn decode(kind: &str, payload: &str) -> Result<EventKind, CodecError> {
    use EventKind::*;
    let mut r = R(payload.split('\t'));
    let t = |r: &mut R| r.n().map(TaskId);
    let ev = match kind {
        "project_registered" => ProjectRegistered {
            project: ProjectId(r.n()?),
            name: r.s()?,
        },
        "task_filed" => TaskFiled {
            project: ProjectId(r.n()?),
            title: r.s()?,
            priority: priority_from(r.raw()?)?,
            trust: trust_from(r.raw()?)?,
            explored: r.b()?,
            proposed: r.b()?,
            blockers: r.ids()?,
            not_before: r.ts()?,
            body: r.opt_s()?,
        },
        "approved" => Approved { task: t(&mut r)? },
        "rejected" => Rejected { task: t(&mut r)? },
        "admitted" => {
            let task = t(&mut r)?;
            let entry = if r.more() {
                let def = r.s()?;
                let step = r.s()?;
                let state = state_from(r.raw()?)?;
                let route =
                    Route::from_name(r.raw()?).ok_or_else(|| CodecError("bad route".into()))?;
                Some(Entry {
                    def,
                    step,
                    state,
                    route,
                })
            } else {
                None
            };
            Admitted { task, entry }
        }
        "step_succeeded" => {
            let task = t(&mut r)?;
            let next = if r.more() {
                let step = r.s()?;
                Some(Next {
                    step,
                    state: state_from(r.raw()?)?,
                })
            } else {
                None
            };
            StepSucceeded { task, next }
        }
        "sent_back" => SentBack {
            task: t(&mut r)?,
            reason: r.s()?,
            to_step: r.opt_s()?,
        },
        "exploration_requested" => ExplorationRequested {
            task: t(&mut r)?,
            reason: r.s()?,
            to_step: r.opt_s()?,
        },
        "explore_anyway" => ExploreAnyway {
            task: t(&mut r)?,
            step: r.s()?,
        },
        "question_asked" => QuestionAsked {
            task: t(&mut r)?,
            question: r.s()?,
        },
        "answered" => Answered {
            task: t(&mut r)?,
            decision: DecisionId(r.n()?),
            answer: r.s()?,
            step: r.opt_s()?,
        },
        "step_failed" => StepFailed {
            task: t(&mut r)?,
            reason: r.s()?,
            class: FailClass::parse(&r.opt_s()?),
        },
        "retried" => Retried {
            task: t(&mut r)?,
            to: state_from(r.raw()?)?,
            step: r.opt_s()?,
            auto: r.opt_s()? == "auto",
        },
        "canceled" => Canceled {
            task: t(&mut r)?,
            delete_branch: r.b()?,
        },
        "closed_no_change" => ClosedNoChange {
            task: t(&mut r)?,
            reason: r.s()?,
        },
        "paused" => Paused { task: t(&mut r)? },
        "resumed" => Resumed { task: t(&mut r)? },
        "wait_started" => WaitStarted {
            task: t(&mut r)?,
            on: r.s()?,
            class: Some(r.opt_s()?).filter(|c| !c.is_empty()),
        },
        "wait_ended" => WaitEnded { task: t(&mut r)? },
        "blocker_added" => BlockerAdded {
            task: t(&mut r)?,
            blocker: t(&mut r)?,
        },
        "blocker_removed" => BlockerRemoved {
            task: t(&mut r)?,
            blocker: t(&mut r)?,
        },
        "blockers_changed" => BlockersChanged {
            task: t(&mut r)?,
            released: r.ids()?,
            added: r.ids()?,
        },
        "repinned" => Repinned {
            task: t(&mut r)?,
            from: r.s()?,
            to: r.s()?,
            changed: r.s()?,
        },
        "revived" => Revived { task: t(&mut r)? },
        "tracker_edited" => TrackerEdited {
            task: t(&mut r)?,
            title: r.s()?,
            body: r.s()?,
            priority: priority_from(&r.s()?)?,
        },
        "tick" => {
            // The only event with no fields; its payload is empty.
            return if payload.is_empty() {
                Ok(Tick)
            } else {
                err("trailing fields")
            };
        }
        other => return err(format!("unknown event kind {other:?}")),
    };
    r.done()?;
    Ok(ev)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples() -> Vec<EventKind> {
        use EventKind::*;
        let nasty = "tab\there\nnew\\line\r\u{1F600} ünï";
        let t = TaskId(7);
        vec![
            ProjectRegistered {
                project: ProjectId(1),
                name: nasty.into(),
            },
            TaskFiled {
                project: ProjectId(1),
                title: nasty.into(),
                body: format!("body {nasty}"),
                priority: Priority::Urgent,
                trust: TrustSource::OwnerAgent,
                explored: true,
                proposed: false,
                blockers: vec![TaskId(1), TaskId(30)],
                not_before: Some(Timestamp(99)),
            },
            TaskFiled {
                project: ProjectId(2),
                title: String::new(),
                body: String::new(),
                priority: Priority::Low,
                trust: TrustSource::Outside,
                explored: false,
                proposed: true,
                blockers: vec![],
                not_before: None,
            },
            Approved { task: t },
            Rejected { task: t },
            Admitted {
                task: t,
                entry: None,
            },
            Admitted {
                task: t,
                entry: Some(Entry {
                    def: "abc123".into(),
                    step: nasty.into(),
                    state: State::Developing,
                    route: Route::Clarity,
                }),
            },
            StepSucceeded {
                task: t,
                next: None,
            },
            StepSucceeded {
                task: t,
                next: Some(Next {
                    step: "test-fast".into(),
                    state: State::Testing,
                }),
            },
            StepSucceeded {
                task: t,
                next: Some(Next {
                    step: String::new(),
                    state: State::Landed,
                }),
            },
            SentBack {
                task: t,
                reason: nasty.into(),
                to_step: "develop".into(),
            },
            ExplorationRequested {
                task: t,
                reason: String::new(),
                to_step: String::new(),
            },
            ExploreAnyway {
                task: t,
                step: "explore".into(),
            },
            QuestionAsked {
                task: t,
                question: nasty.into(),
            },
            Answered {
                task: t,
                decision: DecisionId(3),
                answer: nasty.into(),
                step: "develop".into(),
            },
            StepFailed {
                task: t,
                reason: nasty.into(),
                class: None,
            },
            Retried {
                task: t,
                to: State::Review,
                step: "review".into(),
                auto: false,
            },
            Retried {
                task: t,
                to: State::Testing,
                step: "test".into(),
                auto: true,
            },
            Canceled {
                task: t,
                delete_branch: true,
            },
            ClosedNoChange {
                task: t,
                reason: nasty.into(),
            },
            Paused { task: t },
            Resumed { task: t },
            WaitStarted {
                task: t,
                on: nasty.into(),
                class: Some(nasty.into()),
            },
            WaitEnded { task: t },
            BlockerAdded {
                task: t,
                blocker: TaskId(2),
            },
            BlockerRemoved {
                task: t,
                blocker: TaskId(2),
            },
            BlockersChanged {
                task: t,
                released: vec![TaskId(2), TaskId(3)],
                added: vec![],
            },
            Repinned {
                task: t,
                from: "a".into(),
                to: nasty.into(),
                changed: "steps: x\nsuites: y".into(),
            },
            Revived { task: t },
            TrackerEdited {
                task: t,
                title: nasty.into(),
                body: "line\ntwo\t".into(),
                priority: Priority::High,
            },
            Tick,
        ]
    }

    #[test]
    fn every_event_kind_round_trips() {
        let all = samples();
        let mut kinds: Vec<_> = all.iter().map(|k| encode(k).0).collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), 26, "one sample per EventKind variant");
        for k in all {
            let (kind, payload) = encode(&k);
            assert!(!payload.contains('\n'), "payload is one line");
            assert_eq!(decode(kind, &payload).unwrap(), k, "{kind}");
        }
    }

    #[test]
    fn a_task_filed_payload_from_before_bodies_still_decodes() {
        let old = "1\tfirst\tnormal\towner\t0\t0\t\t-";
        match decode("task_filed", old).unwrap() {
            EventKind::TaskFiled { title, body, .. } => {
                assert_eq!((title.as_str(), body.as_str()), ("first", ""))
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn garbage_is_rejected_not_panicked_on() {
        for (k, p) in [
            ("nope", ""),
            ("approved", ""),
            ("approved", "x"),
            ("approved", "1\t2"),
            ("sent_back", "1\tbad\\q"),
            ("retried", "1\tflying"),
            ("task_filed", "1\tt\tnormal\towner\t2\t0\t\t-"),
            ("task_filed", "1\tt\tnormal\towner\t0\t0\t\t-\tb\textra"),
        ] {
            assert!(decode(k, p).is_err(), "{k} {p:?}");
        }
    }
}
