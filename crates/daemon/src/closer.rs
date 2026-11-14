// SPDX-License-Identifier: GPL-3.0-or-later

//! An explorer closes a task with no change (s254, decision 0249).
//!
//! `submit_close` ends the explore step. The task does not close at once: it asks the owner a
//! decision card, "Close #N: <kind>: <evidence>", with the options "Close now" and "Reopen and
//! explore", and waits in a veto window: 24 hours for the owner's own tasks, 2 hours for tasks
//! MASON or its agents filed. When the window ends with no answer the task is closed
//! (`ClosedNoChange`); "Close now" closes it at once; any other answer reopens it, back in
//! exploring with what the owner said. The pending close is a row in the store, so it survives a
//! restart, and a loop looks at the rows.

use crate::daemon::{now_ms, Daemon};
use crate::jobs::Ctx;
use mason_adapter_api::card::{Card, CardOption, Origin};
use mason_core::*;
use mason_store::ExploreClose;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// The label of the option that closes the task at once.
pub const CLOSE_NOW: &str = "Close now";
/// The label of the option that sends it back to exploring.
pub const REOPEN: &str = "Reopen and explore";

/// How long the owner has to veto: tasks MASON or its agents filed get the short window.
pub fn window(d: &Daemon, trust: TrustSource) -> Duration {
    match trust {
        TrustSource::OwnerAgent => d.cfg.close_window_agent,
        _ => d.cfg.close_window_owner,
    }
}

/// What is written on the closed task: why it was closed without a change.
pub fn reason(c: &ExploreClose) -> String {
    format!(
        "closed by exploring, {}{}: {}",
        c.kind,
        c.duplicate_of
            .map_or(String::new(), |n| format!(" of #{n}")),
        c.evidence
    )
}

/// The decision card for a pending close.
pub fn card(task: u64, c: &ExploreClose, hours: u64) -> Card {
    let kind = match c.duplicate_of {
        Some(n) => format!("{} of #{n}", c.kind),
        None => c.kind.clone(),
    };
    let evidence: String = c.evidence.split_whitespace().collect::<Vec<_>>().join(" ");
    let head = format!("Close #{task}: {kind}: ");
    let room = 200usize.saturating_sub(head.chars().count());
    let short: String = evidence.chars().take(room).collect();
    Card {
        ask: format!("{head}{short}"),
        why: format!(
            "Exploring found nothing to build. It closes with no change by itself after {hours} hour(s) unless you reopen it."
        ),
        options: vec![
            CardOption {
                label: CLOSE_NOW.into(),
                recommended: true,
            },
            CardOption {
                label: REOPEN.into(),
                recommended: false,
            },
        ],
        evidence: vec![evidence.chars().take(300).collect()],
        links: vec![],
        key: "explore-close".into(),
        origin: Origin::Agent,
        problems: vec![],
    }
}

/// The step ended with an accepted `submit_close`: record it and ask the owner.
pub fn ask(c: &Ctx, a: &Value) {
    let d = &c.d;
    let now = now_ms();
    let hours = window(d, c.trust).as_secs().div_ceil(3600);
    let close = ExploreClose {
        task: c.task.0,
        kind: a["kind"].as_str().unwrap_or("already_done").to_string(),
        duplicate_of: a["duplicate_of"].as_u64(),
        evidence: a["evidence"].as_str().unwrap_or_default().to_string(),
        deadline_ms: now + window(d, c.trust).as_millis() as u64,
        at_ms: now,
    };
    let text = card(c.task.0, &close, hours).to_text();
    // Written first: a restart between the two still closes it when the window ends, and a
    // question that did not get asked is asked again by the step's own recovery.
    if d.store.put_explore_close(&close).is_err() {
        return c.fail("could not record the close");
    }
    c.emit(EventKind::QuestionAsked {
        task: c.task,
        question: text,
    });
}

/// Close what has waited out its window; forget a close whose task is gone or no longer waiting.
pub fn expire(d: &Arc<Daemon>) {
    let Ok(w) = d.world() else { return };
    let now = now_ms();
    for c in d.store.explore_closes().unwrap_or_default() {
        let Some(t) = w.tasks.get(&TaskId(c.task)) else {
            let _ = d.store.clear_explore_close(c.task);
            continue;
        };
        if t.state.is_terminal() {
            let _ = d.store.clear_explore_close(c.task);
            continue;
        }
        if now < c.deadline_ms || t.state != State::NeedsAttention {
            continue;
        }
        let _ = d.store.clear_explore_close(c.task);
        let _ = d.emit(EventKind::ClosedNoChange {
            task: TaskId(c.task),
            reason: reason(&c),
        });
        tracing::info!(
            task = c.task,
            "closed with no change: its veto window passed"
        );
    }
}

pub fn spawn(d: &Arc<Daemon>) {
    crate::bg::spawn_loop(d, "explore-close", move |weak| {
        let Some(d) = weak.upgrade() else {
            return false;
        };
        let tick = d.cfg.close_tick;
        expire(&d);
        drop(d);
        std::thread::sleep(tick);
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(kind: &str, dup: Option<u64>, evidence: &str) -> ExploreClose {
        ExploreClose {
            task: 7,
            kind: kind.into(),
            duplicate_of: dup,
            evidence: evidence.into(),
            deadline_ms: 0,
            at_ms: 0,
        }
    }

    #[test]
    fn the_card_says_what_closes_and_offers_close_now_and_reopen() {
        let c = card(7, &close("duplicate", Some(4), "same change as #4"), 24);
        assert_eq!(c.ask, "Close #7: duplicate of #4: same change as #4");
        assert_eq!(
            c.options
                .iter()
                .map(|o| o.label.as_str())
                .collect::<Vec<_>>(),
            [CLOSE_NOW, REOPEN]
        );
        assert_eq!(c.options.iter().filter(|o| o.recommended).count(), 1);
        assert!(c.why.contains("24 hour"), "{}", c.why);
        // A long piece of evidence is cut to fit the 200 characters of an ask.
        let long = card(7, &close("already_done", None, &"x".repeat(900)), 2);
        assert!(
            long.ask.chars().count() <= 200,
            "{}",
            long.ask.chars().count()
        );
        assert!(long.ask.starts_with("Close #7: already_done: "));
    }

    #[test]
    fn the_closed_task_says_why() {
        assert_eq!(
            reason(&close("duplicate", Some(4), "same change")),
            "closed by exploring, duplicate of #4: same change"
        );
        assert_eq!(
            reason(&close("not_a_problem", None, "it works")),
            "closed by exploring, not_a_problem: it works"
        );
    }
}
