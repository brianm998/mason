// SPDX-License-Identifier: GPL-3.0-or-later

//! s509: `fast_command` is a suite key.

use mason_definition::parse;

#[test]
fn a_suite_may_declare_a_fast_command() {
    let text = "schema = 1\nstart = \"develop\"\n\n[step.develop]\ntype = \"agent\"\ncategory = \"developing\"\nnext = \"land\"\n\n[step.land]\ntype = \"landing\"\ncategory = \"landing\"\n\n[suite.unit]\ncommand = \"cargo test\"\nparser = \"tap\"\nmin_tests = 1\nlevel = \"unit\"\nfast_command = \"cargo test --lib\"\n";
    let def = parse(text).unwrap_or_else(|p| panic!("{p:?}"));
    let suite = def.testing.suite("unit").unwrap();
    assert_eq!(suite.fast_command.as_deref(), Some("cargo test --lib"));
}
