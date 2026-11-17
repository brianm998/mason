// SPDX-License-Identifier: GPL-3.0-or-later

//! s509: `fast_command` is a suite key, and MASON's own project file declares one for the cargo
//! suite.

use mason_definition::parse;

#[test]
fn mason_declares_a_fast_tier_for_its_cargo_suite() {
    let text = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.mason/mason.toml"
    ))
    .unwrap();
    let def = parse(&text).unwrap_or_else(|p| panic!("{p:?}"));
    let suite = def.testing.suite("cargo-workspace").unwrap();
    let cmd = suite.fast_command.as_deref().unwrap();
    assert!(cmd.contains("--workspace --lib"), "{cmd}");
    assert!(
        cmd.contains("mason-engine") && cmd.contains("mason-scheduler"),
        "{cmd}"
    );
}
