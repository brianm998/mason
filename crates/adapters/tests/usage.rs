// SPDX-License-Identifier: GPL-3.0-or-later

//! Usage snapshots read from recorded Claude streams. `real_*` files are
//! captures; `synthetic_*` files are hand-written and labelled so.

use mason_adapter_api::{
    AgentAdapter, AgentEvent, BillingKind, BlockedWhy, OverageState, UsageSnapshot, WindowState,
};
use mason_adapters::claude::{final_outcome, parse_line, ClaudeAdapter, ParseState};
use mason_adapters::codex::CodexAdapter;

fn load(name: &str) -> (Vec<UsageSnapshot>, ParseState) {
    let text = std::fs::read_to_string(format!(
        "{}/fixtures/claude/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut st = ParseState::default();
    let snaps = text
        .lines()
        .flat_map(|l| parse_line(l, &mut st))
        .filter_map(|e| match e {
            AgentEvent::Usage(u) => Some(u),
            _ => None,
        })
        .collect();
    (snaps, st)
}

fn last(name: &str) -> UsageSnapshot {
    load(name).0.pop().expect("a usage snapshot")
}

#[test]
fn real_captures_give_both_windows_overage_off_and_not_blocked() {
    for (name, five) in [
        ("real_plain.jsonl", 0.23),
        ("real_tool_suspend.jsonl", 0.24),
    ] {
        let u = last(name);
        assert_eq!(u.windows.len(), 2, "{name}");
        let f = u.window("five_hour").unwrap();
        assert_eq!(f.label, "5-hour");
        assert_eq!(f.length_secs, Some(18_000));
        assert_eq!(f.utilization, Some(five));
        assert_eq!(f.resets_at, Some(1_793_944_800));
        assert_eq!(f.state, WindowState::Ok);
        let w = u.window("seven_day").unwrap();
        assert_eq!(w.label, "Weekly");
        assert_eq!(w.utilization, Some(0.6));
        assert_eq!(w.resets_at, Some(1_794_276_000));
        assert_eq!(
            u.overage.state,
            OverageState::Disabled("org_level_disabled".into())
        );
        assert_eq!(u.blocked, None);
        assert_eq!(u.spend_usd, None, "a plan reports no spend");
        assert!(u.session_id.is_some());
        assert_eq!(u.observed_at, None, "the runner stamps it, not the parser");
    }
}

#[test]
fn synthetic_rejected_five_hour_blocks_until_its_reset() {
    let (snaps, st) = load("synthetic_usage_windows.jsonl");
    let u = snaps.last().unwrap();
    assert_eq!(u.windows.len(), 3);
    assert_eq!(u.window("five_hour").unwrap().state, WindowState::Blocked);
    assert_eq!(u.window("seven_day").unwrap().state, WindowState::Ok);
    let b = u.blocked.as_ref().unwrap();
    assert_eq!(b.why, BlockedWhy::Window("five_hour".into()));
    assert_eq!(b.until, Some(1_793_944_800));
    assert_eq!(u.window("seven_day_opus").unwrap().label, "Weekly (Opus)");
    assert!(matches!(
        final_outcome(&st, false, Some(1), ""),
        mason_adapter_api::Outcome::UsageLimit { .. }
    ));
}

#[test]
fn synthetic_rejected_weekly_blocks_by_the_weekly_window() {
    let u = last("synthetic_usage_rejected_weekly.jsonl");
    let b = u.blocked.clone().unwrap();
    assert_eq!(b.why, BlockedWhy::Window("seven_day".into()));
    assert_eq!(b.until, Some(1_794_276_000));
    assert_eq!(u.window("five_hour").unwrap().state, WindowState::Ok);
}

#[test]
fn synthetic_overage_in_use_is_not_blocked_but_is_shown() {
    let u = last("synthetic_usage_overage_in_use.jsonl");
    assert_eq!(u.window("five_hour").unwrap().state, WindowState::Blocked);
    assert_eq!(u.overage.state, OverageState::InUse);
    assert_eq!(u.blocked, None, "extra billing is running, not blocked");
}

#[test]
fn synthetic_overage_used_up_blocks() {
    let u = last("synthetic_usage_overage_used_up.jsonl");
    assert_eq!(u.overage.state, OverageState::UsedUp);
    assert_eq!(u.overage.resets_at, Some(1_796_000_000));
    let b = u.blocked.unwrap();
    assert_eq!(b.why, BlockedWhy::OverageUsedUp);
    assert_eq!(b.until, Some(1_796_000_000));
}

#[test]
fn synthetic_api_run_gives_spend_and_no_windows() {
    let (snaps, st) = load("synthetic_api_run.jsonl");
    assert_eq!(st.billing, BillingKind::Api);
    let u = snaps.last().unwrap();
    assert_eq!(u.spend_usd, Some(0.0123));
    assert!(u.windows.is_empty());
    assert_eq!(u.blocked, None);
}

#[test]
fn a_subscription_run_reports_no_spend() {
    let (snaps, st) = load("real_plain.jsonl");
    assert_eq!(st.billing, BillingKind::Subscription);
    assert!(snaps.iter().all(|u| u.spend_usd.is_none()));
}

#[test]
fn synthetic_api_429_is_a_rate_limit_not_a_plan_limit() {
    let (snaps, st) = load("synthetic_api_rate_limit.jsonl");
    let u = snaps.last().unwrap();
    assert!(u.windows.is_empty());
    let b = u.blocked.as_ref().unwrap();
    assert_eq!(b.why, BlockedWhy::RateLimit);
    assert_eq!(b.until, None);
    assert!(matches!(
        final_outcome(&st, false, Some(1), ""),
        mason_adapter_api::Outcome::UsageLimit { .. }
    ));
}

#[test]
fn an_unknown_window_passes_through_labelled_with_its_id() {
    let mut st = ParseState::default();
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","unifiedWindows":{"thirty_day":{"utilization":0.5,"resetsAt":5,"status":"allowed"},"odd":{"resetsAt":6,"status":"mystery"}}}}"#;
    let Some(AgentEvent::Usage(u)) = parse_line(line, &mut st).pop() else {
        panic!("no snapshot")
    };
    let w = u.window("thirty_day").unwrap();
    assert_eq!((w.label.as_str(), w.length_secs), ("thirty_day", None));
    assert_eq!(w.utilization, Some(0.5));
    let odd = u.window("odd").unwrap();
    assert_eq!(odd.utilization, None, "never estimated");
    assert_eq!(odd.state, WindowState::Blocked, "unsure means blocked");
    assert!(u.blocked.is_some());
}

#[test]
fn a_warning_status_is_a_warning_and_does_not_block() {
    let mut st = ParseState::default();
    let line = r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":9,"rateLimitType":"seven_day"}}"#;
    let Some(AgentEvent::Usage(u)) = parse_line(line, &mut st).pop() else {
        panic!()
    };
    assert_eq!(u.windows.len(), 1);
    assert_eq!(u.windows[0].state, WindowState::Warning);
    assert_eq!(u.windows[0].utilization, None);
    assert_eq!(u.blocked, None);
}

#[test]
fn usage_models_follow_the_billing_kind() {
    let c = ClaudeAdapter::new("/bin/claude", "/bin/mcp", std::env::temp_dir());
    let sub = c.usage_model(BillingKind::Subscription);
    let ids: Vec<_> = sub.windows.iter().map(|w| w.id.as_str()).collect();
    assert_eq!(ids, ["five_hour", "seven_day", "seven_day_opus"]);
    assert!(sub.utilization && sub.overage && !sub.spend);
    let api = c.usage_model(BillingKind::Api);
    assert!(api.windows.is_empty() && api.spend && !api.utilization);
    let x = CodexAdapter::new("/bin/codex", "/bin/mcp", std::env::temp_dir());
    for b in [BillingKind::Subscription, BillingKind::Api] {
        assert!(
            x.usage_model(b).windows.is_empty(),
            "no Codex capture shows windows"
        );
    }
}

#[test]
fn codex_failure_text_gives_a_refusal_with_no_window_and_nothing_invented() {
    let text = std::fs::read_to_string(format!(
        "{}/fixtures/codex/usage_limit.jsonl",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut st = ParseState::new(mason_adapters::claude::Dialect::Codex);
    let snaps: Vec<_> = text
        .lines()
        .flat_map(|l| parse_line(l, &mut st))
        .filter_map(|e| match e {
            AgentEvent::Usage(u) => Some(u),
            _ => None,
        })
        .collect();
    assert_eq!(snaps.len(), 1, "the repeated message is one snapshot");
    let u = &snaps[0];
    assert!(u.windows.is_empty() && u.spend_usd.is_none());
    let b = u.blocked.as_ref().unwrap();
    assert!(matches!(&b.why, BlockedWhy::Unreported(m) if m.contains("3:00 PM")));
    assert_eq!(b.until, None);
}
