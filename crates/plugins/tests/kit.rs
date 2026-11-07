// SPDX-License-Identifier: GPL-3.0-or-later
//! The conformance kit, on the example plugin (which must pass) and on broken copies
//! of it (each must fail by name).

use mason_plugins::kit::{plugin_kit, Check};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/plugins/migration-check")
}

/// A copy of the example with `check.py` changed by replacing `from` with `to`.
fn broken(from: &str, to: &str) -> tempfile::TempDir {
    let t = tempfile::tempdir().unwrap();
    for f in ["manifest.toml", "schema.json", "check.py"] {
        std::fs::copy(example().join(f), t.path().join(f)).unwrap();
    }
    let p = t.path().join("check.py");
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(
        text.contains(from),
        "the example no longer contains {from:?}"
    );
    std::fs::write(&p, text.replace(from, to)).unwrap();
    t
}

fn failing(checks: &[Check]) -> Vec<(&'static str, String)> {
    checks
        .iter()
        .filter(|c| !c.ok)
        .map(|c| (c.name, c.detail.clone()))
        .collect()
}

const T: Duration = Duration::from_secs(2);

#[test]
fn the_example_plugin_passes_every_check() {
    let checks = plugin_kit(&example(), T);
    assert_eq!(failing(&checks), vec![], "{checks:?}");
    let names: Vec<&str> = checks.iter().map(|c| c.name).collect();
    assert_eq!(
        names,
        [
            "manifest",
            "schema file",
            "start",
            "initialize",
            "schema matches",
            "run",
            "refuses bad config",
            "shutdown"
        ]
    );
}

#[test]
fn a_bad_initialize_is_named() {
    let t = broken(
        "\"name\": \"migration-check\"",
        "\"name\": \"someone-else\"",
    );
    let f = failing(&plugin_kit(t.path(), T));
    assert_eq!(f[0].0, "initialize", "{f:?}");
    assert!(f[0].1.contains("someone-else"), "{f:?}");
}

#[test]
fn a_schema_mismatch_is_named() {
    let t = broken(
        "\"config_schema\": SCHEMA",
        "\"config_schema\": {\"type\": \"object\"}",
    );
    let f = failing(&plugin_kit(t.path(), T));
    assert_eq!(
        f.iter().map(|x| x.0).collect::<Vec<_>>(),
        ["schema matches"],
        "{f:?}"
    );
}

#[test]
fn a_plugin_that_accepts_bad_config_is_named() {
    let t = broken(
        "return None, {\"code\": -32602, \"message\": \"; \".join(problems)}",
        "pass",
    );
    let f = failing(&plugin_kit(t.path(), T));
    assert!(f.iter().any(|x| x.0 == "refuses bad config"), "{f:?}");
}

#[test]
fn a_timeout_is_named_and_the_plugin_is_stopped() {
    let t = broken(
        "def run_step(params):\n",
        "def run_step(params):\n    import time\n    time.sleep(30)\n",
    );
    let started = std::time::Instant::now();
    let f = failing(&plugin_kit(t.path(), Duration::from_secs(3)));
    assert!(started.elapsed() < Duration::from_secs(15));
    let run = f
        .iter()
        .find(|x| x.0 == "run")
        .unwrap_or_else(|| panic!("{f:?}"));
    assert!(run.1.contains("no answer within"), "{f:?}");
}

#[test]
fn a_plugin_that_does_not_shut_down_is_named() {
    let t = broken(
        "            reply(id_, None)\n            return\n",
        "            reply(id_, None)\n            import time\n            time.sleep(30)\n",
    );
    let f = failing(&plugin_kit(t.path(), Duration::from_secs(3)));
    assert_eq!(
        f.iter().map(|x| x.0).collect::<Vec<_>>(),
        ["shutdown"],
        "{f:?}"
    );
    assert!(f[0].1.contains("did not exit"), "{f:?}");
}

#[test]
fn a_missing_program_and_a_bad_manifest_are_named() {
    let t = broken("#!/usr/bin/env python3", "#!/no/such/interpreter");
    let f = failing(&plugin_kit(t.path(), T));
    assert_eq!(f[0].0, "start", "{f:?}");
    let t = tempfile::tempdir().unwrap();
    std::fs::write(t.path().join("manifest.toml"), "name = 1").unwrap();
    assert_eq!(failing(&plugin_kit(t.path(), T))[0].0, "manifest");
}
