// SPDX-License-Identifier: GPL-3.0-or-later

//! The tier and the real model in use, as every monitor and the command line
//! show it: one display string made here (from [`mason_definition::tiers`]'s
//! reverse lookup), plus its parts. What was launched comes from the
//! `step_launches` rows; a step that has not started shows what it is expected
//! to use, marked as such.

use crate::daemon::{now_ms, Daemon};
use mason_core::{ProjectId, Task};
use mason_definition::layers::{resolve, Resolved, RuntimeOverride};
use mason_definition::tiers::{self, Note, Tier};
use mason_protocol::ModelUse;
use mason_store::LaunchRow;

/// The model to show: what the CLI reported, when that is not just the asked-for
/// name written out longer (`opus` asked, `claude-opus-4-1` reported is the same
/// model); else what was asked for. Empty: the CLI's own default.
fn shown_model(row: &LaunchRow) -> String {
    let (asked, said) = (row.model.to_lowercase(), row.reported_model.to_lowercase());
    if said.is_empty() || (!asked.is_empty() && said.contains(&asked)) {
        row.model.clone()
    } else {
        row.reported_model.clone()
    }
}

fn why_of(row: &LaunchRow, shown: &str) -> String {
    let mut why = format!("model: {}; effort: {}", row.model_from, row.effort_from);
    if !row.moved_up_from.is_empty() && row.model_from.starts_with("escalated") {
        why.push_str(&format!(
            "; was {} before the last round",
            row.moved_up_from
        ));
    } else if !row.moved_up_from.is_empty() {
        why.push_str(&format!("; moved up the ladder from {}", row.moved_up_from));
    }
    if shown != row.model {
        why.push_str(&format!(
            "; asked for {}, the CLI reported {}",
            if row.model.is_empty() {
                "its default"
            } else {
                &row.model
            },
            row.reported_model
        ));
    }
    why
}

fn non_empty(s: &str) -> Option<&str> {
    Some(s).filter(|s| !s.is_empty())
}

/// The parts and the display string of one launch row.
pub fn use_of(row: &LaunchRow, expected: bool) -> ModelUse {
    let shown = shown_model(row);
    let display = tiers::describe(
        Tier::parse(&row.tier),
        &row.provider,
        non_empty(&shown),
        non_empty(&row.effort),
        &Note {
            retry: (row.retry_n > 0).then_some((row.retry_n, row.retry_max)),
            overridden: row.overridden,
            expected,
        },
    );
    ModelUse {
        display,
        tier: row.tier.clone(),
        level: row.level.clone(),
        why: why_of(row, &shown),
        model: shown,
        effort: row.effort.clone(),
        provider: row.provider.clone(),
        kind: row.kind.clone(),
        retry_n: row.retry_n,
        retry_max: row.retry_max,
        expected,
        run: row.run,
        requested_model: row.model.clone(),
        reported_model: row.reported_model.clone(),
        overridden: row.overridden,
        at_ms: row.at_ms,
    }
}

/// What `step` would launch for this task now: the five layers, the provider's
/// mapping as it is today, then the real model and effort.
pub fn would_launch(
    d: &Daemon,
    def: &mason_definition::Definition,
    pid: ProjectId,
    task: u64,
    step: &str,
    task_overrides: &[(String, String)],
) -> Option<(LaunchRow, Resolved)> {
    let s = def.step(step)?;
    if s.kind != mason_definition::StepType::Agent {
        return None;
    }
    let project = d.project(pid)?;
    let now = now_ms();
    let runtime: Vec<RuntimeOverride> = d
        .store
        .overrides(&project.id, now)
        .unwrap_or_default()
        .into_iter()
        .map(|o| RuntimeOverride {
            key: o.key,
            value: o.value,
            owner: o.owner,
            expires_at: o.expires_at,
        })
        .collect();
    let res = resolve(def, s, &d.overlay(pid), &runtime, task_overrides, now).ok()?;
    let agent = project.agent();
    let eff = crate::tiermap::effective(&d.cfg.config_dir, agent.provider, &project.overlay_text);
    let l = tiers::launch(&res, &eff.mapping, &agent.ladder);
    let row = LaunchRow {
        task,
        step: step.to_string(),
        kind: "expected".into(),
        tier: l.tier.map(|t| t.name().to_string()).unwrap_or_default(),
        level: l.level.name().to_string(),
        req_model: l.mapped.model.clone().unwrap_or_default(),
        req_effort: l.mapped.effort.clone().unwrap_or_default(),
        model: l.model.clone().unwrap_or_default(),
        effort: l.effort.clone().unwrap_or_default(),
        provider: crate::tiermap::name(agent.provider).into(),
        model_from: l.model_from.clone(),
        effort_from: l.effort_from.clone(),
        retry_max: agent.max_retries,
        overridden: l.overridden,
        ..Default::default()
    };
    Some((row, res))
}

/// The editor's inspector for an agent step: what it would launch now under the
/// current provider, with the source of each part.
pub fn inspect(
    d: &Daemon,
    def: &mason_definition::Definition,
    pid: ProjectId,
    step: &str,
) -> Option<serde_json::Value> {
    let (row, res) = would_launch(d, def, pid, 0, step, &[])?;
    let tier = Tier::parse(&row.tier);
    let level = mason_definition::tiers::Level::parse(&row.level).unwrap_or_default();
    let non_empty = |s: &str| Some(s.to_string()).filter(|s| !s.is_empty());
    let line = tiers::describe_request(
        tier,
        level,
        &row.provider,
        non_empty(&row.model).as_deref(),
        non_empty(&row.effort).as_deref(),
    );
    Some(serde_json::json!({
        "line": line,
        "provider": row.provider,
        "tier": {"value": row.tier, "from": res.tier.as_ref().map(|a| a.source.label())},
        "level": {"value": row.level, "from": res.level.as_ref().map(|a| a.source.label())},
        "model": {"value": row.model, "from": row.model_from},
        "effort": {"value": row.effort, "from": row.effort_from},
    }))
}

/// The use to show for a task sitting in `step`: its latest launch of the latest
/// run of that step, else what it is expected to use.
pub fn current(
    d: &Daemon,
    def: &mason_definition::Definition,
    t: &Task,
    latest_run: Option<u64>,
) -> Option<ModelUse> {
    if let Some(run) = latest_run {
        if let Some(l) = d
            .store
            .launches(run)
            .ok()
            .and_then(|v| v.into_iter().last())
        {
            return Some(use_of(&l, false));
        }
    }
    let overrides = d.store.task_overrides(t.id.0).unwrap_or_default();
    would_launch(d, def, t.project, t.id.0, &t.step, &overrides).map(|(r, _)| use_of(&r, true))
}

/// Every launch of a run, oldest first.
pub fn history(d: &Daemon, run: u64) -> Vec<ModelUse> {
    d.store
        .launches(run)
        .unwrap_or_default()
        .iter()
        .map(|l| use_of(l, false))
        .collect()
}

/// The use to show for a task in a step state; `None` for any other state, and
/// for steps that are not agents.
pub fn task_use(d: &Daemon, w: &mason_engine::World, t: &Task) -> Option<ModelUse> {
    if !t.state.is_step() {
        return None;
    }
    let def = d
        .pinned_def(&t.def)
        .filter(|_| !t.def.is_empty())
        .unwrap_or_else(|| d.current_def(t.project));
    let run = w
        .step_runs
        .values()
        .filter(|r| r.task == t.id && r.name == t.step)
        .map(|r| r.id.0)
        .max();
    current(d, &def, t, run)
}

/// A [`ModelUse`] as JSON, for `mason task show --json` and `mason status --json`.
pub fn json_of(m: &ModelUse) -> serde_json::Value {
    serde_json::json!({
        "display": m.display, "tier": m.tier, "level": m.level, "model": m.model,
        "effort": m.effort, "provider": m.provider, "why": m.why, "kind": m.kind,
        "retry_n": m.retry_n, "retry_max": m.retry_max, "expected": m.expected,
        "run": m.run, "requested_model": m.requested_model,
        "reported_model": m.reported_model, "overridden": m.overridden, "at_ms": m.at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> LaunchRow {
        LaunchRow {
            run: 4,
            kind: "launch".into(),
            tier: "deep".into(),
            level: "medium".into(),
            model: "opus".into(),
            effort: "medium".into(),
            provider: "claude".into(),
            model_from: "the provider file mapping".into(),
            effort_from: "the provider file mapping".into(),
            retry_max: 2,
            ..Default::default()
        }
    }

    #[test]
    fn a_launch_is_shown_as_the_tier_and_the_real_model() {
        let u = use_of(&row(), false);
        assert_eq!(u.display, "deep (Opus medium)");
        assert_eq!(
            (u.tier.as_str(), u.model.as_str(), u.effort.as_str()),
            ("deep", "opus", "medium")
        );
        assert!(
            u.why.contains("model: the provider file mapping"),
            "{}",
            u.why
        );
        assert!(!u.expected && u.run == 4);
    }

    #[test]
    fn expected_retry_and_override_are_marked() {
        assert_eq!(use_of(&row(), true).display, "deep (Opus medium, expected)");
        let mut r = row();
        r.kind = "retry".into();
        r.retry_n = 1;
        r.moved_up_from = "sonnet".into();
        let u = use_of(&r, false);
        assert_eq!(u.display, "deep (Opus medium, retry 1 of 2)");
        assert!(u.why.contains("moved up the ladder from sonnet"));
        r.retry_n = 0;
        r.overridden = true;
        assert_eq!(use_of(&r, false).display, "deep (Opus medium, override)");
        r.tier.clear();
        assert_eq!(use_of(&r, false).display, "Opus medium (override)");
    }

    #[test]
    fn the_cli_default_and_the_models_it_reports() {
        let mut r = row();
        r.provider = "codex".into();
        r.model.clear();
        r.effort = "high".into();
        assert_eq!(use_of(&r, false).display, "deep (Codex default, high)");
        // The CLI says which model it really used: that one is shown, the request kept beside it.
        r.reported_model = "gpt-5".into();
        let u = use_of(&r, false);
        assert_eq!(u.display, "deep (Gpt-5 high)");
        assert_eq!(
            (u.requested_model.as_str(), u.reported_model.as_str()),
            ("", "gpt-5")
        );
        // `opus` asked, the long name of the same model reported: the alias stays.
        let mut r = row();
        r.reported_model = "claude-opus-4-1-20250805".into();
        assert_eq!(use_of(&r, false).display, "deep (Opus medium)");
        // A different model than asked for is shown as what it is.
        r.reported_model = "claude-sonnet-4-5".into();
        let u = use_of(&r, false);
        assert_eq!(u.display, "deep (Claude-sonnet-4-5 medium)");
        assert!(
            u.why
                .contains("asked for opus, the CLI reported claude-sonnet-4-5"),
            "{}",
            u.why
        );
        let mut light = row();
        light.tier = "light".into();
        light.model = "haiku".into();
        light.effort.clear();
        assert_eq!(use_of(&light, false).display, "light (Haiku)");
    }
}
