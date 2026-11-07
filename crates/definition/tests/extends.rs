// SPDX-License-Identifier: GPL-3.0-or-later

//! A step may extend another: it takes what it does not set itself.

use mason_definition::{parse, validate, Capabilities};

const BASE: &str = r#"schema = 1
start = "develop"

[step.develop]
type = "agent"
category = "developing"
agent = "deep"
budget = "4h"
next = "test"

[step.test]
type = "test"
category = "testing"
max_rounds = 3
on_red = "develop"
next = "land"

[step.land]
type = "landing"
category = "landing"
"#;

#[test]
fn a_step_that_extends_takes_what_it_does_not_set() {
    let text = format!("{BASE}\n[step.test-slow]\nextends = \"test\"\nmax_rounds = 5\n");
    let d = parse(&text).unwrap();
    let s = d.step("test-slow").unwrap();
    assert_eq!(s.kind.name(), "test");
    assert_eq!(s.category, mason_core::State::Testing);
    assert_eq!((s.max_rounds, s.on_red.as_deref(), s.next.clone()), (Some(5), Some("develop"), vec!["land".to_string()]));
    // The one it extends is untouched.
    assert_eq!(d.step("test").unwrap().max_rounds, Some(3));
}

#[test]
fn extends_follows_a_chain_and_the_nearest_wins() {
    let text = format!("{BASE}\n[step.a]\nextends = \"develop\"\nbudget = \"1h\"\n\n[step.b]\nextends = \"a\"\nagent = \"fast\"\n");
    let d = parse(&text).unwrap();
    let b = d.step("b").unwrap();
    assert_eq!(b.agent.as_deref(), Some("fast"));
    assert_eq!(b.budget, Some(std::time::Duration::from_secs(3600)));
    assert_eq!(b.kind.name(), "agent");
}

#[test]
fn a_missing_parent_and_a_loop_are_problems_on_the_extends_key() {
    let e = parse(&format!("{BASE}\n[step.x]\nextends = \"ghost\"\n")).unwrap_err();
    assert!(e.iter().any(|p| p.path == "step.x.extends" && p.message.contains("`ghost` is not a step")), "{e:?}");
    let e = parse(&format!("{BASE}\n[step.p]\nextends = \"q\"\n\n[step.q]\nextends = \"p\"\n")).unwrap_err();
    assert!(e.iter().any(|p| p.message.contains("extends itself")), "{e:?}");
    let e = parse(&format!("{BASE}\n[step.s]\nextends = \"s\"\n")).unwrap_err();
    assert!(e.iter().any(|p| p.message.contains("extends itself")), "{e:?}");
}

#[test]
fn an_extending_step_is_checked_like_any_other() {
    // It extends a step with a loop but sets no bound of its own: it inherits the bound.
    let text = format!("{BASE}\n[step.again]\nextends = \"test\"\nnext = \"land\"\n");
    let d = parse(&text).unwrap();
    assert!(validate(&d, &Capabilities::now()).iter().all(|p| !p.path.starts_with("step.again.max_rounds")));
}
