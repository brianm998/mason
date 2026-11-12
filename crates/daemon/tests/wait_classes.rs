// SPDX-License-Identifier: GPL-3.0-or-later

//! Waits are recorded with what they were for, and the digest totals them by that: one row per
//! class, not one line per queue position.

mod support;
use mason_core::*;
use support::*;

const DEF_COMPILE: &str = r#"schema = 1
start = "develop"

[resource.compile]
count = 1

[step.develop]
type = "agent"
category = "developing"
next = "test"

[step.test]
type = "test"
category = "testing"
uses = { compile = 1 }
max_rounds = 2
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

#[test]
fn two_tasks_behind_one_compile_slot_show_as_one_compile_row() {
    let s = Scratch::new("test -f hello.txt", unique_script());
    s.push_files(&[(".mason/mason.toml", DEF_COMPILE)], "definition");
    let d = s.start();
    // Someone outside holds the one slot, so both tasks' test steps queue behind it.
    let map = [("compile".to_string(), 1u32)].into_iter().collect();
    let id = d.broker.lease_open("interactive", &map);
    wait_for("the lease to be granted", 30, || {
        matches!(
            d.broker.lease_poll(id),
            mason_daemon::sched::LeaseState::Granted { .. }
        )
    });
    let a = file_task(&d, "First");
    let b = file_task(&d, "Second");
    wait_for("both tasks to wait", 90, || {
        d.world()
            .map(|w| w.waits.contains_key(&a) && w.waits.contains_key(&b))
            .unwrap_or(false)
    });
    // New waits carry the class, set from the request and not read from the text.
    let w = d.world().unwrap();
    for t in [a, b] {
        assert_eq!(
            w.waits[&t].class.as_deref(),
            Some("compile"),
            "{:?}",
            w.waits[&t]
        );
    }
    d.broker.lease_close(id);
    wait_state(&d, a, State::Landed, 120);
    wait_state(&d, b, State::Landed, 120);

    let now = mason_daemon::daemon::now_ms();
    let digest = mason_daemon::introspect::digest(&d, None, 0, now + 1);
    let rows = digest["wait_by_class"].as_array().unwrap();
    let compile: Vec<_> = rows.iter().filter(|r| r["class"] == "compile").collect();
    assert_eq!(compile.len(), 1, "{rows:?}");
    assert!(compile[0]["count"].as_u64().unwrap() >= 1, "{rows:?}");
    let text = digest.to_string();
    assert!(!text.contains("in line"), "{text}");
    assert!(!text.contains("held by"), "{text}");
}
