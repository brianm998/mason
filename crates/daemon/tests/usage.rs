// SPDX-License-Identifier: GPL-3.0-or-later

//! Stage 64 end to end: what a provider reports is kept and shown. A real
//! daemon runs tasks with the fake agent emitting usage snapshots; the view
//! shows both windows with percentages and resets, the same figures survive a
//! restart (with their age), a window without a percentage shows none, an api
//! account shows spend and no windows, tokens are attributed to the right
//! project, task, step and model, and `mason usage --json` is the view.

mod support;
use mason_core::*;
use mason_daemon::views::Ctx;
use mason_daemon::Daemon;
use mason_protocol::UsageView;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::sync::Arc;
use support::*;

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
}

/// Both turns report limits (5-hour with a percentage, Weekly with none).
fn script() -> Value {
    let rl = |n: u64| {
        json!({"rate_limit": {"status": "allowed", "windows": {
            "five_hour": {"utilization": 0.82, "resetsAt": now() + n},
            "seven_day": {"resetsAt": now() + 3 * 86400}}}})
    };
    let mut explore = explore_turn().as_array().unwrap().clone();
    explore.insert(0, rl(4200));
    let mut develop = develop_turn().as_array().unwrap().clone();
    develop.insert(0, rl(4200));
    json!({"steps": {"explore": [explore], "develop": [develop]}})
}

fn view(d: &Arc<Daemon>) -> UsageView {
    let w = d.world().unwrap();
    Ctx::new(d, &w).usage()
}

fn serve(d: &Arc<Daemon>) -> u16 {
    let d2 = d.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let _ = rt.block_on(mason_daemon::http::serve(d2));
    });
    let info = d.cfg.info_path();
    let mut port = 0;
    wait_for("the socket to be served", 20, || {
        std::fs::read_to_string(&info)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .and_then(|v| v["port"].as_u64())
            .map(|p| port = p as u16)
            .is_some()
    });
    port
}

fn get(port: u16, path: &str) -> Value {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(s, "GET {path} HTTP/1.0\r\nHost: x\r\n\r\n").unwrap();
    let mut body = String::new();
    s.read_to_string(&mut body).unwrap();
    serde_json::from_str(body.split("\r\n\r\n").nth(1).unwrap()).unwrap()
}

fn account<'a>(v: &'a UsageView, name: &str) -> &'a mason_protocol::ProviderStatus {
    v.providers.iter().find(|p| p.name == name).unwrap()
}

#[test]
fn windows_tokens_restart_and_cli() {
    let s = Scratch::new("test -f hello.txt", script());
    let d = s.start();
    let t = file_task(&d, "Report usage");
    wait_state(&d, t, State::Landed, 60);

    let v = view(&d);
    let a = account(&v, "claude");
    assert!(a.has_snapshot);
    assert!(
        a.age.starts_with("just now") || a.age.starts_with("as of"),
        "{}",
        a.age
    );
    let five = a.windows.iter().find(|w| w.id == "five_hour").unwrap();
    assert!(five.reported && five.has_percent);
    assert_eq!(five.percent.round() as u32, 82);
    assert!(
        five.has_reset && five.resets_text.starts_with("resets in 1 h"),
        "{}",
        five.resets_text
    );
    assert!(five.short_text.contains("82%"), "{}", five.short_text);
    // The weekly window came with no percentage: state and reset only.
    let week = a.windows.iter().find(|w| w.id == "seven_day").unwrap();
    assert!(week.reported && !week.has_percent && week.has_reset);
    assert_eq!(week.state, "ok");
    assert!(
        v.top_bar[0].starts_with("claude 5-hour 82%, resets 1 h"),
        "{:?}",
        v.top_bar
    );
    assert_eq!(a.doing, "nothing");
    // Not an api account: no spend.
    assert!(a.spend.is_none());

    // Tokens: one row per step attempt, with project, task, step and model.
    let runs = d.store.usage_runs_since(0).unwrap();
    let mut steps: Vec<&str> = runs.iter().map(|r| r.step.as_str()).collect();
    steps.sort();
    assert_eq!(steps, ["develop", "explore"]);
    for r in &runs {
        assert_eq!(
            (r.project.as_str(), r.task, r.account.as_str()),
            ("scratch", t.0, "claude")
        );
        assert!(r.model.is_some());
        assert_eq!(
            (r.input, r.output, r.cache_read, r.cache_create),
            (Some(4), Some(50), Some(2000), Some(100))
        );
    }
    let wk = a.tokens_week.as_ref().unwrap();
    assert_eq!(wk.by_project.len(), 1);
    assert_eq!(wk.by_project[0].name, "scratch");
    assert!(wk.by_project[0].has_input && wk.by_project[0].input == 8);
    assert_eq!(wk.top_tasks[0].task, t.0);
    let detail = {
        let w = d.world().unwrap();
        Ctx::new(&d, &w).task_detail(t.0).unwrap()
    };
    assert_eq!(detail.step_tokens.len(), 2);

    // A restart keeps the figures, with their age.
    drop(d);
    let d = s.start();
    let v2 = view(&d);
    let a2 = account(&v2, "claude");
    assert!(a2.has_snapshot && a2.as_of > 0);
    assert!(!a2.age.is_empty());
    let five2 = a2.windows.iter().find(|w| w.id == "five_hour").unwrap();
    assert_eq!(five2.percent.round() as u32, 82);
    assert_eq!(five2.resets_at, five.resets_at);
    assert!(
        !a2.windows
            .iter()
            .find(|w| w.id == "seven_day")
            .unwrap()
            .has_percent
    );
    assert!(!five2.age.is_empty());

    // `mason usage --json` is the view GET /usage returns.
    let port = serve(&d);
    for _ in 0..3 {
        let http = get(port, "/usage");
        let cli: Value = serde_json::from_str(&s.mason_ok(&["usage", "--json"])).unwrap();
        if http == cli {
            let one: Value =
                serde_json::from_str(&s.mason_ok(&["usage", "--account", "claude", "--json"]))
                    .unwrap();
            assert_eq!(one["providers"].as_array().unwrap().len(), 1);
            let text = s.mason_ok(&["usage"]);
            assert!(text.contains("5-hour: 82%"), "{text}");
            assert!(text.contains("Weekly: no percentage reported"), "{text}");
            let o = s.mason(&["usage", "--account", "nope"]);
            assert!(!o.status.success());
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    panic!("`mason usage --json` never equalled GET /usage");
}

#[test]
fn an_api_account_shows_spend_against_its_budget_and_no_windows() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    std::fs::write(
        s.config_dir.join("providers.toml"),
        "[provider.apiacct]\nadapter = \"claude\"\nbilling = \"api\"\nmonthly_budget_usd = 50\n",
    )
    .unwrap();
    s.write_project("scratch", "test -f hello.txt", "account = \"apiacct\"");
    let d = s.start();
    let t = file_task(&d, "Spend something");
    wait_state(&d, t, State::Landed, 60);
    let v = view(&d);
    let a = account(&v, "apiacct");
    assert!(a.windows.is_empty());
    let sp = a.spend.as_ref().unwrap();
    assert!(sp.has_spend && sp.has_budget);
    assert_eq!(sp.budget_usd, 50.0);
    assert!((sp.month_usd - 0.002).abs() < 1e-9, "{}", sp.month_usd);
    assert!((sp.today_usd - 0.002).abs() < 1e-9);
}

#[test]
fn a_provider_that_reports_nothing_shows_nothing() {
    let s = Scratch::new("test -f hello.txt", happy_script());
    let d = s.start();
    let v = view(&d);
    let a = account(&v, "claude");
    assert!(!a.has_snapshot && a.age.is_empty());
    // The adapter's windows are listed, as not reported: no percentage, no reset.
    assert!(a
        .windows
        .iter()
        .all(|w| !w.reported && !w.has_percent && !w.has_reset));
    assert!(v.top_bar.is_empty());
    assert!(a.tokens_week.as_ref().unwrap().by_model.is_empty());
}
