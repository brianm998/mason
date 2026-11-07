// SPDX-License-Identifier: GPL-3.0-or-later

//! The conformance kit for plugins: any implementation, in any language, must pass
//! it. It reads the folder, starts the plugin and checks, in order:
//!
//! 1. `manifest`: `manifest.toml` is valid;
//! 2. `schema file`: the file `config_schema` names is a usable JSON Schema;
//! 3. `start`: the program starts;
//! 4. `initialize`: it answers with the right shape, its name, protocol and
//!    `provides` match the manifest;
//! 5. `schema matches`: the `config_schema` it reports equals the file's;
//! 6. `run`: a step with schema-valid settings gets `{passed, summary}` in time;
//! 7. `refuses bad config`: settings the schema forbids get an error answer;
//! 8. `shutdown`: it answers `shutdown` and exits.
//!
//! A check that cannot run because an earlier one failed is reported as failed, saying why.

use crate::host::{self, CallError, Launch, Plugin, StepRequest};
use crate::manifest::{parse_manifest, MANIFEST_FILE};
use crate::schema;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub name: &'static str,
    pub ok: bool,
    pub detail: String,
}

fn pass(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        ok: true,
        detail: detail.into(),
    }
}

fn fail(name: &'static str, detail: impl Into<String>) -> Check {
    Check {
        name,
        ok: false,
        detail: detail.into(),
    }
}

/// Run the kit on an extension folder. `timeout` is how long each call may take.
pub fn plugin_kit(dir: &Path, timeout: Duration) -> Vec<Check> {
    let mut out = vec![];
    let blocked = |name: &'static str, why: &str| fail(name, format!("not run: {why}"));
    let text = match std::fs::read_to_string(dir.join(MANIFEST_FILE)) {
        Ok(t) => t,
        Err(e) => {
            return vec![fail(
                "manifest",
                format!("cannot read {MANIFEST_FILE}: {e}"),
            )];
        }
    };
    let m = match parse_manifest(&text) {
        Ok(m) => {
            out.push(pass("manifest", format!("{} {}", m.name, m.version)));
            m
        }
        Err(e) => return vec![fail("manifest", e.join("; "))],
    };

    let file_schema: Option<Value> = match &m.config_schema {
        None => {
            out.push(pass("schema file", "none declared; settings are free-form"));
            None
        }
        Some(f) => match std::fs::read_to_string(dir.join(f))
            .map_err(|e| format!("cannot read {f}: {e}"))
            .and_then(|t| {
                serde_json::from_str::<Value>(&t).map_err(|e| format!("{f} is not JSON: {e}"))
            }) {
            Ok(s) => {
                let p = schema::check_schema(&s);
                if p.is_empty() {
                    out.push(pass("schema file", f.clone()));
                    Some(s)
                } else {
                    out.push(fail("schema file", format!("{f}: {}", p.join("; "))));
                    None
                }
            }
            Err(e) => {
                out.push(fail("schema file", e));
                None
            }
        },
    };
    let declared = file_schema
        .clone()
        .unwrap_or_else(|| json!({"type": "object"}));

    let launch = Launch {
        dir: dir.to_path_buf(),
        run: m.run.clone(),
        log: None,
    };
    let mut p = match Plugin::spawn(&launch) {
        Ok(p) => {
            out.push(pass("start", m.run.join(" ")));
            p
        }
        Err(e) => {
            out.push(fail(
                "start",
                format!("cannot start `{}`: {e}", m.run.join(" ")),
            ));
            for n in [
                "initialize",
                "schema matches",
                "run",
                "refuses bad config",
                "shutdown",
            ] {
                out.push(blocked(n, "the plugin did not start"));
            }
            return out;
        }
    };

    // initialize
    let init = match host::initialize(&mut p, timeout) {
        Ok(i) => {
            let mut wrong = vec![];
            if i.name != m.name {
                wrong.push(format!(
                    "name `{}` is not the manifest's `{}`",
                    i.name, m.name
                ));
            }
            if i.protocol != crate::PROTOCOL {
                wrong.push(format!(
                    "protocol {} is not {}",
                    i.protocol,
                    crate::PROTOCOL
                ));
            }
            let (mut a, mut b) = (i.provides.clone(), m.provides());
            a.sort();
            b.sort();
            if a != b {
                wrong.push(format!(
                    "provides {:?} is not the manifest's {:?}",
                    i.provides,
                    m.provides()
                ));
            }
            if wrong.is_empty() {
                out.push(pass("initialize", format!("{} {}", i.name, i.version)));
            } else {
                out.push(fail("initialize", wrong.join("; ")));
            }
            Some(i)
        }
        Err(e) => {
            out.push(fail("initialize", e));
            None
        }
    };
    let Some(init) = init else {
        for n in ["schema matches", "run", "refuses bad config", "shutdown"] {
            out.push(blocked(n, "initialize failed"));
        }
        return out;
    };

    // schema matches
    if init.config_schema == declared {
        out.push(pass(
            "schema matches",
            "initialize reports the file's schema",
        ));
    } else {
        out.push(fail(
            "schema matches",
            "the config_schema from initialize differs from the schema file the manifest names",
        ));
    }

    // run
    let req = |config: Value| StepRequest {
        step: "kit".into(),
        step_type: m.step_types.first().cloned().unwrap_or_default(),
        config,
        task: 0,
        title: "conformance kit".into(),
        worktree: dir.to_string_lossy().into_owned(),
        env: json!({}),
    };
    let good = schema::sample_config(&declared);
    let mut alive = true;
    match host::step_run(&mut p, &req(good.clone()), timeout) {
        Ok(r) => out.push(pass("run", format!("passed={} {}", r.passed, r.summary))),
        Err(e) => {
            if matches!(e, CallError::Timeout(_) | CallError::Closed(_)) {
                alive = false;
            }
            out.push(fail(
                "run",
                format!("step.run with settings {good} failed: {e}"),
            ));
        }
    }

    // refuses bad config
    match (alive, schema::bad_config(&declared)) {
        (false, _) => out.push(blocked(
            "refuses bad config",
            "the plugin stopped answering during run",
        )),
        (true, None) => out.push(pass(
            "refuses bad config",
            "the schema forbids nothing to test with",
        )),
        (true, Some(bad)) => match host::step_run(&mut p, &req(bad.clone()), timeout) {
            Err(CallError::Rpc(e)) => out.push(pass("refuses bad config", e.message)),
            Ok(_) => out.push(fail(
                "refuses bad config",
                format!("step.run accepted settings the schema forbids: {bad}"),
            )),
            Err(e) => {
                alive = false;
                out.push(fail(
                    "refuses bad config",
                    format!("step.run with {bad}: {e}"),
                ));
            }
        },
    }

    // shutdown
    if alive {
        match p.shutdown(timeout) {
            Ok(()) => out.push(pass("shutdown", "answered and exited")),
            Err(e) => out.push(fail("shutdown", e)),
        }
    } else {
        out.push(blocked("shutdown", "the plugin stopped answering earlier"));
    }
    out
}
