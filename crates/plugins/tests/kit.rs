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

/// How long the kit waits for each answer. Generous on purpose: a copy of the plugin is
/// started fresh by seven tests at once, and on a busy macOS machine the first exec of a
/// new file (the system scanning it) alone takes half a second or more. A passing test
/// never waits this long.
const T: Duration = Duration::from_secs(10);

/// For the two tests that need the kit to give up on a plugin that sleeps for 30 s.
const SHORT: Duration = Duration::from_secs(6);

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
            "progress",
            "refuses bad config",
            "shutdown"
        ]
    );
    let progress = checks.iter().find(|c| c.name == "progress").unwrap();
    assert!(progress.detail.contains("well-formed"), "{progress:?}");
}

#[test]
fn a_malformed_step_progress_is_named() {
    let t = broken(
        "progress(message=f\"checking {config['directory']}\")",
        "progress(done=\"0\", message=\"x\")",
    );
    let f = failing(&plugin_kit(t.path(), T));
    assert_eq!(
        f.iter().map(|x| x.0).collect::<Vec<_>>(),
        ["progress"],
        "{f:?}"
    );
    assert!(f[0].1.contains("done"), "{f:?}");
}

#[test]
fn a_protocol_1_plugin_runs_and_must_not_send_progress() {
    // Declared protocol 1 on both sides, the example is a version 1 plugin that still sends
    // step.progress: the host ignores it, and the kit says it needs protocol 2.
    let t = broken("\"protocol\": 2,", "\"protocol\": 1,");
    let m = t.path().join("manifest.toml");
    let text = std::fs::read_to_string(&m).unwrap();
    std::fs::write(&m, text.replace("protocol = 2", "protocol = 1")).unwrap();
    let f = failing(&plugin_kit(t.path(), T));
    assert_eq!(
        f.iter().map(|x| x.0).collect::<Vec<_>>(),
        ["progress"],
        "{f:?}"
    );
    assert!(f[0].1.contains("protocol 2"), "{f:?}");
    // Without the notifications it passes, as every version 1 plugin did.
    let quiet = broken(
        "def progress(**fields):",
        "def progress(**fields):\n    return",
    );
    let m = quiet.path().join("manifest.toml");
    let text = std::fs::read_to_string(&m).unwrap();
    std::fs::write(&m, text.replace("protocol = 2", "protocol = 1")).unwrap();
    let c = quiet.path().join("check.py");
    let text = std::fs::read_to_string(&c).unwrap();
    std::fs::write(&c, text.replace("\"protocol\": 2,", "\"protocol\": 1,")).unwrap();
    assert_eq!(failing(&plugin_kit(quiet.path(), T)), vec![]);
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
    let f = failing(&plugin_kit(t.path(), SHORT));
    assert!(started.elapsed() < Duration::from_secs(25));
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
    let f = failing(&plugin_kit(t.path(), SHORT));
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
