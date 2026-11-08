// SPDX-License-Identifier: GPL-3.0-or-later

//! Every rule of the validator, with a definition that breaks it and one that
//! does not.

use mason_definition::{parse, validate, Capabilities, Problem};

fn problems(text: &str, caps: &Capabilities) -> Vec<Problem> {
    let d = parse(text).unwrap_or_else(|e| panic!("should parse: {e:?}\n{text}"));
    validate(&d, caps)
}

fn bad(text: &str, want_path: &str, want_msg: &str) {
    let p = problems(text, &Capabilities::everything());
    assert!(
        p.iter()
            .any(|x| x.path == want_path && x.message.contains(want_msg)),
        "wanted {want_path}: {want_msg:?} in {p:?}\n{text}"
    );
}

fn good(text: &str) {
    let p = problems(text, &Capabilities::everything());
    assert!(p.is_empty(), "{p:?}\n{text}");
}

/// A straight line explore, develop, test, land, with `extra` appended and
/// `head` as the top-level keys.
fn line(head: &str, extra: &str) -> String {
    format!(
        r#"schema = 1
start = "explore"
{head}
[step.explore]
type = "agent"
category = "exploring"
next = "develop"
[step.develop]
type = "agent"
category = "developing"
next = "test"
[step.test]
type = "test"
category = "testing"
max_rounds = 2
on_red = "develop"
next = "land"
[step.land]
type = "landing"
category = "landing"
{extra}
"#
    )
}

#[test]
fn valid_definitions_pass() {
    good(&line("", ""));
    good(&line("explored_entry = \"develop\"", ""));
    // A review agent that can send work back, an approval, a command step, a second test step.
    good(
        r#"schema = 1
start = "develop"
[step.develop]
type = "agent"
category = "developing"
tools = ["submit_change", "run_check", "ask"]
next = "check"
[step.check]
type = "command"
category = "testing"
command = "make lint"
next = "test"
[step.test]
type = "test"
category = "testing"
next = "review"
[step.review]
type = "agent"
category = "review"
max_rounds = 2
on_red = "develop"
next = "ok"
[step.ok]
type = "approval"
category = "landing"
next = "land"
[step.land]
type = "landing"
category = "landing"
"#,
    );
}

#[test]
fn entry_points_must_exist_and_explored_entry_must_come_after_exploring() {
    bad(
        &line("", "").replace("start = \"explore\"", "start = \"nope\""),
        "start",
        "`nope` is not a step",
    );
    bad(
        &line("explored_entry = \"nope\"", ""),
        "explored_entry",
        "`nope` is not a step",
    );
    bad(
        &line("explored_entry = \"explore\"", ""),
        "explored_entry",
        "exploring step",
    );
}

#[test]
fn references_must_exist_and_go_forward() {
    bad(
        &line("", "").replace("next = \"develop\"", "next = \"ghost\""),
        "step.explore.next",
        "`ghost` is not a step",
    );
    bad(
        &line("", "").replace("on_red = \"develop\"", "on_red = \"ghost\""),
        "step.test.on_red",
        "`ghost` is not a step",
    );
    bad(
        &line("", "").replace("next = \"test\"", "next = \"explore\""),
        "step.develop.next",
        "going back is what on_red is for",
    );
    bad(
        &line("", "").replace("next = \"test\"", "next = \"develop\""),
        "step.develop.next",
        "its own next step",
    );
}

#[test]
fn every_path_must_reach_a_landing_step() {
    // A step that ends the path without landing strands tasks.
    bad(
        &line("", "").replace("next = \"test\"", ""),
        "step.develop.next",
        "stranded",
    );
    // Nothing may follow landing.
    bad(
        &line("", "").replace(
            "category = \"landing\"",
            "category = \"landing\"\nnext = \"explore\"",
        ),
        "step.land.next",
        "landing ends the task",
    );
    // A step nobody reaches is dead configuration.
    bad(&line("", "[step.orphan]\ntype = \"command\"\ncategory = \"testing\"\ncommand = \"true\"\nnext = \"land\"\n"), "step.orphan", "not reachable");
}

#[test]
fn every_loop_must_be_bounded() {
    bad(
        &line("", "").replace("max_rounds = 2\n", ""),
        "step.test.max_rounds",
        "bound the loop",
    );
    bad(
        &line("", "").replace("on_red = \"develop\"\n", ""),
        "step.test.max_rounds",
        "only meaningful with on_red",
    );
    // A loop through `next` alone has no bound, even within one category.
    let cyc = r#"schema = 1
start = "a"
[step.a]
type = "command"
category = "testing"
command = "true"
next = "b"
[step.b]
type = "command"
category = "testing"
command = "true"
next = "a"
"#;
    let p = problems(cyc, &Capabilities::everything());
    assert!(
        p.iter().any(|x| x.message.contains("unbounded loop")),
        "{p:?}"
    );
    // Only red work goes back, and only to developing steps.
    bad(
        &line("", "").replace("on_red = \"develop\"", "on_red = \"explore\""),
        "step.test.on_red",
        "goes back to a developing step",
    );
    bad(
        &line("", "").replace(
            "next = \"test\"",
            "next = \"test\"\nmax_rounds = 2\non_red = \"develop\"",
        ),
        "step.develop.on_red",
        "only testing, review and landing steps",
    );
}

#[test]
fn categories_types_and_keys_must_fit_together() {
    bad(
        &line("", "").replace(
            "type = \"test\"\ncategory = \"testing\"",
            "type = \"test\"\ncategory = \"developing\"",
        ),
        "step.test.category",
        "cannot be in `developing`",
    );
    bad(
        &line("", "").replace(
            "type = \"landing\"\ncategory = \"landing\"",
            "type = \"landing\"\ncategory = \"review\"",
        ),
        "step.land.category",
        "use landing",
    );
    bad(
        &line("", "").replace(
            "type = \"agent\"\ncategory = \"exploring\"",
            "type = \"agent\"\ncategory = \"exploring\"\ncommand = \"make\"",
        ),
        "step.explore.command",
        "does not apply to agent steps",
    );
    bad(
        &line(
            "",
            "[step.c]\ntype = \"command\"\ncategory = \"testing\"\nnext = \"land\"\n",
        ),
        "step.c.command",
        "needs a `command`",
    );
}

#[test]
fn agent_steps_name_real_profiles_and_tools() {
    bad(
        &line("", "").replace(
            "type = \"agent\"\ncategory = \"exploring\"",
            "type = \"agent\"\ncategory = \"exploring\"\nagent = \"genius\"",
        ),
        "step.explore.agent",
        "no agent profile `genius`",
    );
    good(&line("[agent.genius]\nmodel = \"x\"", "").replace(
        "type = \"agent\"\ncategory = \"exploring\"",
        "type = \"agent\"\ncategory = \"exploring\"\nagent = \"genius\"",
    ));
    bad(&line("", "").replace("type = \"agent\"\ncategory = \"developing\"", "type = \"agent\"\ncategory = \"developing\"\ntools = [\"submit_change\", \"submit_plan\"]"), "step.develop.tools[1]", "not available in a developing step");
    bad(
        &line("", "").replace(
            "type = \"agent\"\ncategory = \"developing\"",
            "type = \"agent\"\ncategory = \"developing\"\ntools = [\"run_check\"]",
        ),
        "step.develop.tools",
        "could never finish",
    );
    bad(
        &line("", "").replace(
            "type = \"agent\"\ncategory = \"exploring\"",
            "type = \"agent\"\ncategory = \"exploring\"\nread_only = false",
        ),
        "step.explore.read_only",
        "always read-only",
    );
}

#[test]
fn resources_must_be_declared() {
    bad(
        &line("", "").replace("max_rounds = 2", "max_rounds = 2\nuses = { phone = 1 }"),
        "step.test.uses.phone",
        "no resource `phone`",
    );
    good(
        &line("", "[resource.phone]\ncount = 1")
            .replace("max_rounds = 2", "max_rounds = 2\nuses = { phone = 1 }"),
    );
}

#[test]
fn schema_versions_are_checked() {
    bad(
        &line("", "").replace("schema = 1", "schema = 2"),
        "schema",
        "schema 2 is not supported",
    );
}

#[test]
fn what_cannot_run_yet_is_refused_when_applying_but_is_valid_language() {
    let gate = line("", "").replace(
        "type = \"test\"\ncategory = \"testing\"",
        "type = \"gate\"\ncategory = \"testing\"",
    );
    good(&gate); // the language has it
    let p = problems(&gate, &Capabilities::now());
    assert!(
        p.iter().any(|x| x.path == "step.test.type"
            && x.message.contains("cannot run yet")
            && x.message.contains("Stage 9")),
        "{p:?}"
    );
    let ci = line("", "").replace(
        "type = \"test\"\ncategory = \"testing\"",
        "type = \"ci\"\ncategory = \"testing\"",
    );
    assert!(problems(&ci, &Capabilities::now())
        .iter()
        .any(|x| x.message.contains("Stage 15")));
    let cond = line("", "").replace(
        "max_rounds = 2",
        "max_rounds = 2\nwhen = \"plan.needs_device\"",
    );
    good(&cond);
    assert!(problems(&cond, &Capabilities::now())
        .iter()
        .any(|x| x.path == "step.test.when"));
    bad(
        &line("", "").replace("max_rounds = 2", "max_rounds = 2\nwhen = \"banana\""),
        "step.test.when",
        "not a condition",
    );
}

const DIAMOND: &str = r#"schema = 1
start = "develop"
[step.develop]
type = "agent"
category = "developing"
next = ["unit", "lint"]
[step.unit]
type = "test"
category = "testing"
next = "gather"
[step.lint]
type = "command"
category = "testing"
command = "make lint"
next = "gather"
[step.gather]
type = "approval"
category = "review"
join = "all-green"
next = "land"
[step.land]
type = "landing"
category = "landing"
"#;

#[test]
fn fan_out_and_joins_must_behave() {
    good(DIAMOND);
    // It is valid language but cannot run yet.
    let p = problems(DIAMOND, &Capabilities::now());
    assert!(
        p.iter()
            .any(|x| x.message.contains("fan-out cannot run yet")),
        "{p:?}"
    );
    // Branches that never meet.
    bad(&DIAMOND.replace("[step.lint]\ntype = \"command\"\ncategory = \"testing\"\ncommand = \"make lint\"\nnext = \"gather\"", "[step.lint]\ntype = \"command\"\ncategory = \"testing\"\ncommand = \"make lint\"\nnext = \"land\""), "step.develop.next", "reach");
    // A join nobody fans out to.
    bad(
        &line("", "").replace("next = \"land\"", "next = \"land\"\njoin = \"first-red\""),
        "step.test.join",
        "no fan-out leads to it",
    );
    // A step inside the fan-out also reached from outside.
    let outside = DIAMOND.replace("[step.land]", "[step.side]\ntype = \"command\"\ncategory = \"testing\"\ncommand = \"x\"\nnext = \"unit\"\n[step.land]").replace("start = \"develop\"", "start = \"develop\"\nexplored_entry = \"side\"");
    let p = problems(&outside, &Capabilities::everything());
    assert!(
        p.iter().any(|x| x.message.contains("also reached from")),
        "{p:?}"
    );
    // Landing inside a branch ends it before it rejoins.
    let lands = DIAMOND.replace("[step.lint]\ntype = \"command\"\ncategory = \"testing\"\ncommand = \"make lint\"\nnext = \"gather\"", "[step.lint]\ntype = \"command\"\ncategory = \"testing\"\ncommand = \"make lint\"\nnext = \"land\"");
    assert!(!problems(&lands, &Capabilities::everything()).is_empty());
    // Duplicates in a next list.
    let dup = DIAMOND.replace("next = [\"unit\", \"lint\"]", "next = [\"unit\", \"unit\"]");
    assert!(problems(&dup, &Capabilities::everything())
        .iter()
        .any(|x| x.message.contains("listed twice")));
}

#[test]
fn problems_always_say_where_and_why() {
    let p = problems(
        &line("", "")
            .replace("max_rounds = 2\n", "")
            .replace("next = \"develop\"", "next = \"ghost\""),
        &Capabilities::everything(),
    );
    assert!(p.len() >= 2);
    for x in &p {
        assert!(!x.message.is_empty());
        assert!(
            x.path.starts_with("step.") || x.path.is_empty() || x.path == "start",
            "{x:?}"
        );
    }
}

#[test]
fn efforts_are_levels_or_the_running_providers_own_and_anything_else_is_refused() {
    let claude = Capabilities::everything().with_efforts(
        ["low", "medium", "high", "xhigh", "max"]
            .map(String::from)
            .to_vec(),
    );
    let codex = Capabilities::everything().with_efforts(
        ["minimal", "low", "medium", "high"]
            .map(String::from)
            .to_vec(),
    );
    let with = |e: &str| {
        mason_definition::builtin::BUILTIN.replace(
            "agent = \"light\"",
            &format!("agent = \"light\"\neffort = \"{e}\""),
        )
    };
    let text = with("high");
    assert!(
        text.contains("effort = \"high\""),
        "the fixture has an agent step to put effort on"
    );
    assert!(problems(&text, &claude).is_empty());
    // A provider's own level passes there, is refused under the other, and warns.
    let max = with("max");
    assert!(problems(&max, &claude).is_empty());
    let p = problems(&max, &codex);
    assert!(
        p.iter().any(|x| x.path.ends_with(".effort")
            && x.message.contains("low, medium, high")
            && x.message.contains("minimal")),
        "{p:?}"
    );
    let d = parse(&max).unwrap();
    let w = mason_definition::validate::warnings(&d, &claude);
    assert!(
        w.iter().any(|x| x.message.contains("ties this pipeline")),
        "{w:?}"
    );
    assert!(mason_definition::validate::warnings(&parse(&text).unwrap(), &claude).is_empty());
    // With no provider known, nothing is judged.
    assert!(problems(&with("ultra"), &Capabilities::everything()).is_empty());
    // An explicit model is a warning, not an error.
    let m = mason_definition::builtin::BUILTIN
        .replace("agent = \"light\"", "agent = \"light\"\nmodel = \"opus\"");
    assert!(problems(&m, &claude).is_empty());
    let w = mason_definition::validate::warnings(&parse(&m).unwrap(), &claude);
    assert!(w.iter().any(|x| x.path.ends_with(".model")), "{w:?}");
}
