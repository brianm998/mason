// SPDX-License-Identifier: GPL-3.0-or-later

//! The Project page's side of the daemon (s1232): one catalogue of every setting a project's
//! registration file holds, with the value as written, the effective value, where it comes from,
//! whether it can be written and what a change does. Clients draw the page from this description,
//! so a key added here needs no client change.
//!
//! - Served additively under `catalogue` in `GET /projects/{name}/settings`; written through
//!   `POST` on the same route with `{"set": {"<key>": value-or-null}}` (`null` unsets the key,
//!   back to its default), `"preview": true` for the diff alone and `"confirm": true` for a key
//!   that runs a program or weakens a guard.
//! - A write follows decision 0423: `toml_edit` on the file text (comments survive), the
//!   loading parsers, an atomic write, the reload, and the old text back if the reload refuses.
//! - Secrets are never returned or accepted: only `env:NAME` and `file:/path` references, and
//!   names that look secret show as "set in file, not shown".
//! - Keys that run a program or weaken a guard are refused from a paired device, an owner's
//!   included (decision 1222); a key fixed at start is shown and never written.

use crate::daemon::Daemon;
use serde_json::{json, Map, Value};
use toml_edit::{value, Array, DocumentMut, Item, Table};

pub type Refused = crate::settings::Refused;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    Str,
    Int(i64, i64),
    Frac,
    Bool,
    List,
    Choice(&'static [&'static str]),
    Ref,
}

/// How sensitive a key is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sens {
    Plain,
    /// A secret reference (`env:NAME` or `file:/path`).
    Ref,
    /// Runs a program on this machine.
    Program,
    /// Weakens a guard, or points a credential somewhere else.
    Guard,
    /// Fixed when masond starts.
    Fixed,
}

impl Sens {
    fn name(self) -> &'static str {
        match self {
            Sens::Plain => "plain",
            Sens::Ref => "secret_reference",
            Sens::Program => "runs_a_program",
            Sens::Guard => "weakens_a_guard",
            Sens::Fixed => "fixed_at_start",
        }
    }
    /// Editable only from a monitor on the daemon's own Mac, and only with confirmation.
    pub fn local_only(self) -> bool {
        matches!(self, Sens::Program | Sens::Guard)
    }
}

/// When a change takes effect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Eff {
    /// A step run reads the registration when it starts (`jobs::ctx`), so the next step run of
    /// every task, admitted or not, uses it; a run already going keeps what it started with
    /// (the limits read through `Ctx::live` excepted).
    Step,
    /// Read from the current registration whenever the daemon next needs it.
    Live,
    Restart,
}

impl Eff {
    fn name(self) -> &'static str {
        match self {
            Eff::Step => "next_step_run",
            Eff::Live => "next_use",
            Eff::Restart => "after_restart",
        }
    }
    fn text(self) -> &'static str {
        match self {
            Eff::Step => "the next step run (a run already going keeps what it started with)",
            Eff::Live => "the next time the daemon reads it",
            Eff::Restart => "after a daemon restart",
        }
    }
}

pub struct K {
    pub path: &'static str,
    pub section: &'static str,
    pub ty: Ty,
    pub default: &'static str,
    pub sens: Sens,
    pub eff: Eff,
}

const fn k(
    path: &'static str,
    section: &'static str,
    ty: Ty,
    default: &'static str,
    sens: Sens,
    eff: Eff,
) -> K {
    K {
        path,
        section,
        ty,
        default,
        sens,
        eff,
    }
}

const MODES: &[&str] = &["local", "remote", "favor-local", "favor-remote"];
const DISP: &[&str] = &["proposed", "queued"];
const PROVIDERS: &[&str] = &["claude", "codex"];
const SANDBOXES: &[&str] = &["none", "sandbox-exec", "bubblewrap"];
const NOLIM: i64 = i64::MAX;

use Eff::{Live, Restart, Step};
use Sens::{Fixed, Guard, Plain, Program, Ref as SRef};
use Ty::{Bool, Choice, Frac, Int, List, Str};

/// Every key the parsers read, as of s1232. A `*` stands for a name (a step, a schedule, an
/// environment variable, a provider, a tier, a level).
pub static KEYS: &[K] = &[
    // -- Project
    k("path", "project", Str, "null", Fixed, Restart),
    k("base", "project", Str, "null", Fixed, Restart),
    k("remote", "project", Str, "null", Fixed, Restart),
    k("test", "project", Str, "null", Program, Step),
    k("build", "project", Str, "null", Program, Step),
    // -- Models and agents
    k(
        "provider",
        "models",
        Choice(PROVIDERS),
        "\"claude\"",
        Plain,
        Step,
    ),
    k("account", "models", Str, "null", Plain, Step),
    k("claude_bin", "models", Str, "null", Program, Step),
    k("codex_bin", "models", Str, "null", Program, Step),
    k("model", "models", Str, "null", Plain, Step),
    k("ladder", "models", List, "[]", Plain, Step),
    k("max_retries", "models", Int(0, 5), "1", Plain, Step),
    k("escalate_model", "models", Str, "\"opus\"", Plain, Step),
    k("diagnose", "models", Bool, "true", Plain, Live),
    k("diagnose_model", "models", Str, "null", Plain, Live),
    k("diagnose_effort", "models", Str, "null", Plain, Live),
    k(
        "sandbox",
        "models",
        Choice(SANDBOXES),
        "\"none\"",
        Guard,
        Step,
    ),
    k("agent.user_skills", "models", Bool, "true", Guard, Step),
    k("agent_env.*", "models", Str, "null", Guard, Step),
    k("step.*.model", "models", Str, "null", Plain, Step),
    k("step.*.effort", "models", Str, "null", Plain, Step),
    k("step.*.budget", "models", Str, "null", Plain, Step),
    k(
        "step.*.max_rounds",
        "models",
        Int(1, NOLIM),
        "null",
        Plain,
        Step,
    ),
    k("step.*.command", "models", Str, "null", Program, Step),
    k("step.*.build", "models", Str, "null", Program, Step),
    k("provider.*.*.*.model", "models", Str, "null", Plain, Step),
    k("provider.*.*.*.effort", "models", Str, "null", Plain, Step),
    // -- Limits and scheduling
    k(
        "step_timeout_s",
        "limits",
        Int(1, NOLIM),
        "14400",
        Plain,
        Step,
    ),
    k(
        "tool_timeout_s",
        "limits",
        Int(1, NOLIM),
        "600",
        Plain,
        Step,
    ),
    k(
        "test_timeout_s",
        "limits",
        Int(1, NOLIM),
        "1800",
        Plain,
        Step,
    ),
    k(
        "agent_idle_alert_s",
        "limits",
        Int(0, NOLIM),
        "2700",
        Plain,
        Step,
    ),
    k(
        "agent_idle_stop_s",
        "limits",
        Int(0, NOLIM),
        "5400",
        Plain,
        Step,
    ),
    k("cont_after_s", "limits", Int(1, NOLIM), "0", Plain, Step),
    k("kill_after_s", "limits", Int(1, NOLIM), "0", Plain, Step),
    k("auto_retry", "limits", Bool, "true", Plain, Live),
    k(
        "priority_jumps_progress",
        "limits",
        Bool,
        "false",
        Plain,
        Live,
    ),
    k(
        "hold_back_secs",
        "limits",
        Int(0, NOLIM),
        "7200",
        Plain,
        Live,
    ),
    k("hold_back_ignore", "limits", List, "[]", Plain, Live),
    k(
        "park_extra",
        "limits",
        Int(0, u32::MAX as i64),
        "null",
        Plain,
        Live,
    ),
    k("small_run_s", "limits", Int(0, NOLIM), "180", Plain, Live),
    // -- Testing and builds
    k(
        "max_test_rounds",
        "testing",
        Int(1, NOLIM),
        "null",
        Plain,
        Step,
    ),
    k("test_reruns", "testing", Int(0, 3), "0", Plain, Step),
    k("rerun_filter", "testing", Str, "null", Plain, Step),
    k("reuse_green", "testing", Bool, "false", Plain, Live),
    k(
        "reuse_full_hours",
        "testing",
        Int(0, NOLIM),
        "24",
        Plain,
        Live,
    ),
    k("task_cache", "testing", Str, "null", Plain, Step),
    k(
        "task_cache_keep_days",
        "testing",
        Int(0, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k("task_cache_seed", "testing", Bool, "false", Plain, Live),
    k("clarity_classifier", "testing", Str, "null", Program, Live),
    k("card_rewriter", "testing", Str, "null", Program, Live),
    // -- Where runs go
    k("placement.mode", "runs", Choice(MODES), "null", Plain, Live),
    k(
        "remote_runs.kind",
        "runs",
        Choice(&["github-actions"]),
        "null",
        Plain,
        Live,
    ),
    k(
        "remote_runs.api",
        "runs",
        Str,
        "\"https://api.github.com\"",
        Guard,
        Live,
    ),
    k("remote_runs.repo", "runs", Str, "null", Plain, Live),
    k("remote_runs.token", "runs", Ty::Ref, "null", SRef, Live),
    k(
        "remote_runs.workflow",
        "runs",
        Str,
        "\"mason-run.yml\"",
        Plain,
        Live,
    ),
    k(
        "remote_runs.runs_on",
        "runs",
        Str,
        "\"ubuntu-latest\"",
        Plain,
        Live,
    ),
    k(
        "remote_runs.pickup_timeout",
        "runs",
        Int(1, NOLIM),
        "1200",
        Plain,
        Live,
    ),
    k(
        "remote_runs.spill_after",
        "runs",
        Int(1, NOLIM),
        "120",
        Plain,
        Live,
    ),
    k(
        "remote_runs.debt_units",
        "runs",
        Int(1, NOLIM),
        "4",
        Plain,
        Live,
    ),
    k(
        "remote_runs.remote_queue_max",
        "runs",
        Int(1, NOLIM),
        "4",
        Plain,
        Live,
    ),
    k(
        "remote_runs.fallback",
        "runs",
        Choice(&["local", "none"]),
        "null",
        Plain,
        Live,
    ),
    k(
        "remote_runs.mode",
        "runs",
        Choice(MODES),
        "null",
        Plain,
        Live,
    ),
    k("remote_runs.test_env.*", "runs", Str, "null", Plain, Live),
    // -- Services
    k("host.kind", "services", Str, "null", Plain, Live),
    k(
        "host.api",
        "services",
        Str,
        "\"https://api.github.com\"",
        Guard,
        Live,
    ),
    k("host.repo", "services", Str, "null", Plain, Live),
    k("host.token", "services", Ty::Ref, "null", SRef, Live),
    k("host.dir", "services", Str, "null", Guard, Live),
    k("host.remote", "services", Str, "null", Guard, Live),
    k("host.checks", "services", Str, "null", Plain, Live),
    k(
        "host.require_approval",
        "services",
        Bool,
        "false",
        Plain,
        Live,
    ),
    k("host.merge_queue", "services", Bool, "false", Plain, Live),
    k("ci.kind", "services", Str, "null", Plain, Live),
    k(
        "ci.api",
        "services",
        Str,
        "\"https://api.github.com\"",
        Guard,
        Live,
    ),
    k("ci.repo", "services", Str, "null", Plain, Live),
    k("ci.token", "services", Ty::Ref, "null", SRef, Live),
    k("ci.workflow", "services", Str, "null", Plain, Live),
    k("ci.trigger", "services", Str, "null", Program, Live),
    k("ci.status", "services", Str, "null", Program, Live),
    k("ci.cancel", "services", Str, "null", Program, Live),
    k("ci.cwd", "services", Str, "\".\"", Guard, Live),
    k("ci.url", "services", Str, "null", Guard, Live),
    k("ci.job", "services", Str, "null", Plain, Live),
    k("ci.user", "services", Str, "null", Plain, Live),
    k("ci.crumb", "services", Bool, "false", Plain, Live),
    k("ci.trigger_url", "services", Str, "null", Guard, Live),
    k("ci.status_url", "services", Str, "null", Guard, Live),
    k("ci.infra_judge", "services", Bool, "false", Plain, Live),
    k("ci.infra_patterns", "services", List, "[]", Plain, Live),
    k("ci.poll_s", "services", Int(1, NOLIM), "20", Plain, Live),
    k(
        "ci.timeout_s",
        "services",
        Int(1, NOLIM),
        "7200",
        Plain,
        Live,
    ),
    k("ci.stuck_after", "services", Str, "1800", Plain, Live),
    k("tracker.kind", "services", Str, "null", Plain, Live),
    k(
        "tracker.api",
        "services",
        Str,
        "\"https://api.github.com\"",
        Guard,
        Live,
    ),
    k("tracker.repo", "services", Str, "null", Plain, Live),
    k("tracker.token", "services", Ty::Ref, "null", SRef, Live),
    k(
        "tracker.webhook_secret",
        "services",
        Ty::Ref,
        "null",
        SRef,
        Live,
    ),
    k(
        "tracker.interval_s",
        "services",
        Int(1, NOLIM),
        "60",
        Plain,
        Live,
    ),
    k("tracker.label", "services", Str, "\"mason\"", Plain, Live),
    k(
        "tracker.import_as",
        "services",
        Choice(DISP),
        "\"proposed\"",
        Plain,
        Live,
    ),
    k(
        "tracker.require_label",
        "services",
        Bool,
        "false",
        Plain,
        Live,
    ),
    // -- Proposals and jobs
    k(
        "proposals.agent_normal",
        "proposals",
        Choice(DISP),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.agent_blocks",
        "proposals",
        Choice(DISP),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.outside_normal",
        "proposals",
        Choice(DISP),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.outside_blocks",
        "proposals",
        Choice(DISP),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.per_task",
        "proposals",
        Int(0, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.per_day",
        "proposals",
        Int(0, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.introspection.low_small",
        "proposals",
        Choice(DISP),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.introspection.low_large",
        "proposals",
        Choice(DISP),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.introspection.high_small",
        "proposals",
        Choice(DISP),
        "null",
        Plain,
        Live,
    ),
    k(
        "proposals.introspection.high_large",
        "proposals",
        Choice(DISP),
        "null",
        Plain,
        Live,
    ),
    k("introspect.enabled", "proposals", Bool, "true", Plain, Live),
    k("introspect.analyst", "proposals", Bool, "true", Plain, Live),
    k(
        "introspect.window",
        "proposals",
        Str,
        "\"24h\"",
        Plain,
        Live,
    ),
    k(
        "introspect.min_samples",
        "proposals",
        Int(1, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k(
        "introspect.loop_per_task",
        "proposals",
        Int(1, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k(
        "introspect.loop_per_project",
        "proposals",
        Int(1, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k(
        "introspect.same_signature",
        "proposals",
        Int(1, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k(
        "introspect.force_skips",
        "proposals",
        Int(1, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k(
        "introspect.no_result_rate",
        "proposals",
        Frac,
        "null",
        Plain,
        Live,
    ),
    k(
        "introspect.wait_share",
        "proposals",
        Frac,
        "null",
        Plain,
        Live,
    ),
    k(
        "introspect.landing_idle_ms",
        "proposals",
        Int(1, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k("triage.enabled", "proposals", Bool, "false", Plain, Live),
    k(
        "triage.sample_percent",
        "proposals",
        Int(0, 100),
        "20",
        Plain,
        Live,
    ),
    k("triage.command", "proposals", Str, "null", Program, Live),
    k("schedule.*.every", "proposals", Str, "null", Plain, Live),
    k("schedule.*.title", "proposals", Str, "null", Plain, Live),
    k("schedule.*.body", "proposals", Str, "null", Plain, Live),
    k(
        "schedule.*.command",
        "proposals",
        Str,
        "null",
        Program,
        Live,
    ),
    k("schedule.*.parser", "proposals", Str, "null", Plain, Live),
    k(
        "schedule.*.results_file",
        "proposals",
        Str,
        "null",
        Plain,
        Live,
    ),
    k(
        "schedule.*.timeout_s",
        "proposals",
        Int(1, NOLIM),
        "null",
        Plain,
        Live,
    ),
    k("schedule.*.workflow", "proposals", Str, "null", Plain, Live),
    k("schedule.*.ref", "proposals", Str, "null", Plain, Live),
    k("schedule.*.run_name", "proposals", Str, "null", Plain, Live),
    k("schedule.*.at", "proposals", Str, "null", Plain, Live),
    k(
        "schedule.*.paths_from",
        "proposals",
        Str,
        "null",
        Plain,
        Live,
    ),
    k("schedule.*.group_by", "proposals", Str, "null", Plain, Live),
    k("self.protect", "proposals", List, "[]", Fixed, Restart),
    k(
        "self.max_open",
        "proposals",
        Int(1, NOLIM),
        "3",
        Fixed,
        Restart,
    ),
    k(
        "self.per_week",
        "proposals",
        Int(1, NOLIM),
        "10",
        Fixed,
        Restart,
    ),
    k(
        "self.max_steps",
        "proposals",
        Int(1, NOLIM),
        "40",
        Fixed,
        Restart,
    ),
    k(
        "self.period_h",
        "proposals",
        Int(1, NOLIM),
        "24",
        Fixed,
        Restart,
    ),
    k(
        "self.cooldown_h",
        "proposals",
        Int(1, NOLIM),
        "24",
        Fixed,
        Restart,
    ),
    k(
        "self.holds",
        "proposals",
        Choice(&["review", "owner"]),
        "\"review\"",
        Fixed,
        Restart,
    ),
];

/// Keys that exist in the file and in the parsers but are not catalogued as editable rows: the
/// `[provider] use` spelling of `provider`, and `[schedule.<n>] kind` (a schedule's kind is its
/// identity; the page edits existing schedules).
const ALIASES: &[&str] = &["provider.use", "schedule.*.kind"];

fn matches_pattern(pattern: &str, path: &str) -> bool {
    let (p, q): (Vec<&str>, Vec<&str>) = (pattern.split('.').collect(), path.split('.').collect());
    p.len() == q.len() && p.iter().zip(&q).all(|(a, b)| *a == "*" || a == b)
}

pub fn find(path: &str) -> Option<&'static K> {
    KEYS.iter().find(|k| matches_pattern(k.path, path))
}

fn recognised(path: &str) -> bool {
    find(path).is_some() || ALIASES.iter().any(|a| matches_pattern(a, path))
}

/// Names that look like they hold a secret.
fn secret_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["TOKEN", "SECRET", "KEY", "PASS", "CREDENTIAL", "AUTH"]
        .iter()
        .any(|w| n.contains(w))
}

/// A key's value is hidden when its own name or (for a table of names) its last segment looks
/// secret, or when it is a reference-typed key that is not a reference.
fn hide(k: Option<&K>, path: &str, v: &Value) -> bool {
    let last = path.rsplit('.').next().unwrap_or(path);
    let named = path.starts_with("agent_env.") || path.starts_with("remote_runs.test_env.");
    if named && secret_name(last) {
        return true;
    }
    match k {
        Some(k) if k.sens == SRef => !v
            .as_str()
            .is_some_and(|s| s.starts_with("env:") || s.starts_with("file:")),
        Some(_) => false,
        None => secret_name(last),
    }
}

const HIDDEN: &str = "set in file, not shown";

fn json_of_item(item: &Item) -> Value {
    match item.as_value() {
        Some(toml_edit::Value::String(s)) => json!(s.value()),
        Some(toml_edit::Value::Integer(i)) => json!(i.value()),
        Some(toml_edit::Value::Float(f)) => json!(f.value()),
        Some(toml_edit::Value::Boolean(b)) => json!(b.value()),
        Some(toml_edit::Value::Array(a)) => Value::Array(
            a.iter()
                .map(|x| match x {
                    toml_edit::Value::String(s) => json!(s.value()),
                    other => json!(other.to_string().trim()),
                })
                .collect(),
        ),
        Some(other) => json!(other.to_string().trim()),
        None => Value::Null,
    }
}

/// The item at a dotted path, with `[provider] use` standing for `provider` when `provider` is
/// a table.
fn written_at<'a>(doc: &'a DocumentMut, path: &str) -> Option<&'a Item> {
    let mut cur: &Item = doc.as_item();
    let segs: Vec<&str> = path.split('.').collect();
    for s in &segs {
        cur = cur.as_table_like()?.get(s)?;
    }
    if path == "provider" && cur.as_table_like().is_some() {
        return cur.as_table_like()?.get("use");
    }
    Some(cur)
}

/// Every leaf of a document as `(dotted path, item)`.
fn leaves<'a>(t: &'a dyn toml_edit::TableLike, prefix: &str, out: &mut Vec<(String, &'a Item)>) {
    for (k, v) in t.iter() {
        let p = if prefix.is_empty() {
            k.to_string()
        } else {
            format!("{prefix}.{k}")
        };
        match v.as_table_like() {
            Some(sub) => leaves(sub, &p, out),
            None => out.push((p, v)),
        }
    }
}

fn type_json(ty: Ty) -> Value {
    match ty {
        Ty::Str => json!({"kind": "string"}),
        Ty::Int(lo, hi) => {
            json!({"kind": "integer", "min": lo, "max": if hi == NOLIM { Value::Null } else { json!(hi) }})
        }
        Ty::Frac => json!({"kind": "number", "min": 0, "max": 1}),
        Ty::Bool => json!({"kind": "boolean"}),
        Ty::List => json!({"kind": "list"}),
        Ty::Choice(c) => json!({"kind": "choice", "choices": c}),
        Ty::Ref => json!({"kind": "reference", "hint": "env:NAME or file:/path"}),
    }
}

/// Who is asking, as far as the settings are concerned.
#[derive(Clone, Debug)]
pub struct Caller {
    pub who: String,
    /// The request came through a paired device (LAN or relay), not from this Mac.
    pub paired: bool,
}

impl Caller {
    pub fn local(who: &str) -> Caller {
        Caller {
            who: who.to_string(),
            paired: false,
        }
    }
}

/// Why this caller cannot write this key, if it cannot.
fn why_not(d: &Daemon, c: &Caller, k: &K) -> Option<String> {
    if k.sens == Fixed {
        return Some("fixed when masond starts: change the file and restart the daemon".into());
    }
    if !crate::attention::is_owner(d, &c.who) {
        return Some(
            "only the owner can change settings (viewers see everything and change nothing)".into(),
        );
    }
    if k.sens.local_only() && c.paired {
        return Some(
            "this key runs a program or weakens a guard, so it is editable only from a monitor on the daemon's own Mac".into(),
        );
    }
    None
}

fn effective_special(cfg: &crate::config::ProjectCfg, path: &str, legacy: &Value) -> Option<Value> {
    let agent = cfg.agent();
    let intro = cfg.introspect();
    Some(match path {
        "provider" => json!(crate::tiermap::name(agent.provider)),
        "account" => json!(cfg.account),
        "escalate_model" => json!(agent.escalate_model),
        "max_retries" => json!(agent.max_retries),
        "ladder" => json!(agent.ladder),
        "sandbox" => json!(format!("{:?}", agent.sandbox)
            .to_lowercase()
            .replace("sandboxexec", "sandbox-exec")),
        "agent.user_skills" => json!(!agent.user_skills_off),
        "introspect.enabled" => json!(intro.enabled),
        "introspect.analyst" => json!(intro.analyst),
        "introspect.min_samples" => json!(intro.th.min_samples),
        "introspect.loop_per_task" => json!(intro.th.loop_per_task),
        "introspect.loop_per_project" => json!(intro.th.loop_per_project),
        "introspect.same_signature" => json!(intro.th.same_signature),
        "introspect.force_skips" => json!(intro.th.force_skips),
        "introspect.no_result_rate" => json!(intro.th.no_result_rate),
        "introspect.wait_share" => json!(intro.th.wait_share),
        "introspect.landing_idle_ms" => json!(intro.th.landing_idle_ms),
        "triage.enabled" => json!(intro.triage.enabled),
        "triage.sample_percent" => json!(intro.triage.percent),
        "triage.command" => json!(intro.triage.command),
        p if p.starts_with("proposals.introspection.") => {
            legacy["proposals"]["introspection"][&p["proposals.introspection.".len()..]].clone()
        }
        p if p.starts_with("proposals.") => legacy["proposals"][&p["proposals.".len()..]].clone(),
        "placement.mode" => json!(cfg.services.placement),
        "remote_runs.mode" => json!(cfg.services.remote.as_ref().and_then(|r| r.mode.clone())),
        _ => return None,
    })
}

fn entry(
    d: &Daemon,
    c: &Caller,
    cfg: &crate::config::ProjectCfg,
    doc: &DocumentMut,
    k: &K,
    path: &str,
    legacy: &Value,
) -> Value {
    let item = written_at(doc, path);
    let mut written = item.map(json_of_item);
    let shown_hidden = written.as_ref().is_some_and(|v| hide(Some(k), path, v));
    if shown_hidden {
        written = Some(json!(HIDDEN));
    }
    let default: Value = serde_json::from_str(k.default).unwrap_or(Value::Null);
    let (effective, source) = if shown_hidden {
        (json!(HIDDEN), "project file")
    } else if let Some(v) = written.as_ref().filter(|_| item.is_some()) {
        let eff = match (k.ty, v.as_i64()) {
            (Ty::Int(lo, hi), Some(n)) => json!(n.clamp(lo, hi)),
            _ => effective_special(cfg, path, legacy).unwrap_or_else(|| v.clone()),
        };
        (eff, "project file")
    } else {
        (
            effective_special(cfg, path, legacy).unwrap_or_else(|| default.clone()),
            "default",
        )
    };
    let why = why_not(d, c, k);
    json!({
        "key": path, "section": k.section, "file": "registration file",
        "type": type_json(k.ty), "default": default, "written": written,
        "effective": effective, "source": source,
        "writable": why.is_none(), "why_not": why,
        "can_unset": why.is_none() && item.is_some(),
        "sensitivity": k.sens.name(), "needs_confirmation": k.sens.local_only(),
        "effect": k.eff.name(), "effect_text": k.eff.text(),
    })
}

const SECTIONS: &[(&str, &str)] = &[
    ("project", "Project"),
    ("models", "Models and agents"),
    ("limits", "Limits and scheduling"),
    ("testing", "Testing and builds"),
    ("runs", "Where runs go"),
    ("services", "Services"),
    ("proposals", "Proposals and jobs"),
];

/// The catalogue for one project: every section's entries, the models view and the keys the
/// catalogue does not know.
pub fn catalogue(d: &Daemon, name: &str, c: &Caller, legacy: &Value) -> Result<Value, Refused> {
    let pid = d
        .project_id_by_name(name)
        .ok_or_else(|| (404, format!("no project {name:?}")))?;
    let cfg = d
        .project(pid)
        .ok_or_else(|| (404, format!("no project {name:?}")))?;
    let file = d
        .cfg
        .config_dir
        .join("projects")
        .join(format!("{name}.toml"));
    let text = std::fs::read_to_string(&file).map_err(|e| (404, format!("{name}: {e}")))?;
    let doc: DocumentMut = text.parse().map_err(|e| (400, format!("{name}: {e}")))?;
    let def = d.current_def(pid);

    // Which concrete paths to show: every non-wildcard key, plus each wildcard key for the
    // names the file or the definition has.
    let mut leaf = vec![];
    leaves(doc.as_table(), "", &mut leaf);
    let mut paths: Vec<(&K, String)> = vec![];
    for key in KEYS {
        if !key.path.contains('*') {
            paths.push((key, key.path.to_string()));
        }
    }
    for (p, _) in &leaf {
        if let Some(key) = find(p).filter(|k| k.path.contains('*')) {
            if !key.path.starts_with("provider.") {
                paths.push((key, p.clone()));
            }
        }
    }
    for s in &def.steps {
        let attrs: &[&str] = match s.kind.name() {
            "agent" => &["model", "effort", "budget", "max_rounds"],
            "test" => &["command", "build", "budget", "max_rounds"],
            _ => &[],
        };
        for a in attrs {
            let p = format!("step.{}.{a}", s.name);
            if !paths.iter().any(|(_, x)| *x == p) {
                paths.push((find(&p).unwrap_or(&KEYS[0]), p));
            }
        }
    }
    let mut sections = vec![];
    for (id, title) in SECTIONS {
        let mut entries: Vec<Value> = paths
            .iter()
            .filter(|(k, _)| k.section == *id)
            .map(|(k, p)| entry(d, c, cfg, &doc, k, p, legacy))
            .collect();
        entries.sort_by(|a, b| a["key"].as_str().cmp(&b["key"].as_str()));
        entries.dedup_by(|a, b| a["key"] == b["key"]);
        sections.push(json!({"id": id, "title": title, "entries": entries}));
    }

    let unrecognised: Vec<Value> = leaf
        .iter()
        .filter(|(p, _)| !recognised(p))
        .map(|(p, item)| {
            let v = json_of_item(item);
            let v = if hide(None, p, &v) { json!(HIDDEN) } else { v };
            json!({"key": p, "written": v, "writable": false,
                   "why_not": "the daemon does not know this key; edit the file by hand"})
        })
        .collect();

    Ok(json!({
        "project": {
            "name": name,
            "file": file.display().to_string(),
            "definition": {"hash": def.hash, "origin": d.current_origin(pid).label()},
            "rename": format!("/projects/{name}/rename"),
        },
        "sections": sections,
        "models": models(d, pid, cfg, &doc, c),
        "unrecognised": unrecognised,
        "caller": {"owner": crate::attention::is_owner(d, &c.who), "paired": c.paired},
    }))
}

fn entry_json(e: &mason_definition::tiers::Entry) -> Value {
    json!({"model": e.model, "effort": e.effort})
}

/// The Models and agents view: the real model of each agent step, the diagnosis session, the
/// last-round escalation and the tier grid with where each cell comes from.
fn models(
    d: &Daemon,
    pid: mason_core::ProjectId,
    cfg: &crate::config::ProjectCfg,
    doc: &DocumentMut,
    c: &Caller,
) -> Value {
    let agent = cfg.agent();
    let pname = crate::tiermap::name(agent.provider);
    let eff = crate::tiermap::effective(&d.cfg.config_dir, agent.provider, &cfg.overlay_text);
    let def = d.current_def(pid);
    let steps: Vec<Value> = def
        .steps
        .iter()
        .filter_map(|s| {
            crate::launchview::inspect(d, &def, pid, &s.name).map(|mut v| {
                v["step"] = json!(s.name);
                v
            })
        })
        .collect();
    let (dg, dsrc) = crate::tiermap::diagnose_entry(
        &eff,
        agent.provider,
        (cfg.diagnose_model.clone(), cfg.diagnose_effort.clone()),
    );
    let mut grid = vec![];
    for t in mason_definition::tiers::Tier::ALL {
        for l in mason_definition::tiers::Level::ALL {
            let (e, src) = eff
                .mapping
                .get(t, l)
                .map_or((Default::default(), "none"), |(e, s)| {
                    (e.clone(), s.label())
                });
            let base = format!("provider.{pname}.{}.{}", t.name(), l.name());
            let k = find(&format!("{base}.model"));
            let why = k.and_then(|k| why_not(d, c, k));
            grid.push(json!({
                "tier": t.name(), "level": l.name(), "entry": entry_json(&e), "source": src,
                "model_key": format!("{base}.model"), "effort_key": format!("{base}.effort"),
                "editable": why.is_none(),
                "note": if src == "provider file" { "set machine-wide in providers/<provider>.toml; a project entry overrides it here" } else { "" },
            }));
        }
    }
    let accounts: Vec<Value> = crate::providers::load(&d.cfg.config_dir)
        .unwrap_or_default()
        .iter()
        .map(|a| {
            json!({"name": a.name, "provider": crate::providers::adapter_name(a.adapter),
                   "sessions": a.sessions, "monthly_budget_usd": a.monthly_budget_usd,
                   "warn_at": a.warn_at})
        })
        .collect();
    json!({
        "provider": pname,
        "provider_written_as_table": doc.get("provider").is_some_and(|p| p.is_table_like()),
        "steps": steps,
        "diagnose": {"enabled": cfg.diagnose, "entry": entry_json(&dg), "source": dsrc.label()},
        "escalation": {"model": agent.escalate_model, "max_retries": agent.max_retries, "ladder": agent.ladder},
        "grid": grid,
        "accounts": accounts,
        "problems": eff.problems,
    })
}

// ---- writing --------------------------------------------------------------------------------

fn bad(m: impl Into<String>) -> Refused {
    (400, m.into())
}

fn to_item(k: &K, path: &str, v: &Value) -> Result<Item, Refused> {
    let must = |what: &str| bad(format!("`{path}` must be {what}"));
    match k.ty {
        Ty::Str => {
            let s = v.as_str().ok_or_else(|| must("a string"))?;
            if s.trim().is_empty() && path != "escalate_model" {
                return Err(must("not empty (unset it to go back to the default)"));
            }
            Ok(value(s))
        }
        Ty::Ref => {
            let s = v.as_str().unwrap_or_default();
            if !(s.starts_with("env:") || s.starts_with("file:")) {
                return Err(bad(format!(
                    "`{path}` is a reference, `env:NAME` or `file:/path`, never the secret itself"
                )));
            }
            Ok(value(s))
        }
        Ty::Int(lo, hi) => {
            let n = v.as_i64().ok_or_else(|| must("a whole number"))?;
            if n < lo || n > hi {
                return Err(bad(format!(
                    "`{path}` must be from {lo}{}",
                    if hi == NOLIM {
                        String::new()
                    } else {
                        format!(" to {hi}")
                    }
                )));
            }
            Ok(value(n))
        }
        Ty::Frac => {
            let n = v.as_f64().ok_or_else(|| must("a number from 0 to 1"))?;
            if !(0.0..=1.0).contains(&n) {
                return Err(must("a number from 0 to 1"));
            }
            Ok(value(n))
        }
        Ty::Bool => Ok(value(v.as_bool().ok_or_else(|| must("true or false"))?)),
        Ty::List => {
            let a = v.as_array().ok_or_else(|| must("a list of strings"))?;
            let mut arr = Array::new();
            for x in a {
                arr.push(x.as_str().ok_or_else(|| must("a list of strings"))?);
            }
            Ok(value(arr))
        }
        Ty::Choice(c) => {
            let s = v.as_str().unwrap_or_default();
            if !c.contains(&s) {
                return Err(bad(format!("`{path}` is one of {}", c.join(", "))));
            }
            Ok(value(s))
        }
    }
}

fn sub<'a>(t: &'a mut Table, key: &str) -> Result<&'a mut Table, Refused> {
    let item = t.entry(key).or_insert_with(|| {
        let mut nt = Table::new();
        nt.set_implicit(true);
        Item::Table(nt)
    });
    // `provider = "codex"` and `[provider.claude]` cannot both be in one file: the string
    // becomes `[provider] use = "codex"`.
    if key == "provider" {
        if let Some(s) = item.as_str().map(String::from) {
            let mut nt = Table::new();
            nt.insert("use", value(s));
            *item = Item::Table(nt);
        }
    }
    item.as_table_mut()
        .ok_or_else(|| bad(format!("`{key}` in the file is not a table")))
}

fn set_at(doc: &mut DocumentMut, path: &str, new: Item) -> Result<(), Refused> {
    let segs: Vec<&str> = path.split('.').collect();
    if path == "provider" {
        let root = doc.as_table_mut();
        if let Some(t) = root.get_mut("provider").and_then(|i| i.as_table_mut()) {
            t.insert("use", new);
        } else {
            root.insert("provider", new);
        }
        return Ok(());
    }
    let (last, parents) = segs.split_last().ok_or_else(|| bad("empty key"))?;
    let mut t = doc.as_table_mut();
    for p in parents {
        t = sub(t, p)?;
    }
    match (
        t.get_mut(last).and_then(|i| i.as_value_mut()),
        new.as_value(),
    ) {
        (Some(old), Some(nv)) => {
            let mut nv = nv.clone();
            *nv.decor_mut() = old.decor().clone();
            *old = nv;
        }
        _ => {
            t.insert(last, new);
        }
    }
    Ok(())
}

fn unset_at(doc: &mut DocumentMut, path: &str) {
    fn go(t: &mut Table, segs: &[&str]) {
        match segs {
            [] => {}
            [last] => {
                t.remove(last);
            }
            [first, rest @ ..] => {
                if let Some(child) = t.get_mut(first).and_then(|i| i.as_table_mut()) {
                    go(child, rest);
                    if child.is_empty() {
                        t.remove(first);
                    }
                }
            }
        }
    }
    if path == "provider" {
        let root = doc.as_table_mut();
        match root.get_mut("provider").and_then(|i| i.as_table_mut()) {
            Some(t) => {
                t.remove("use");
                if t.is_empty() {
                    root.remove("provider");
                }
            }
            None => {
                root.remove("provider");
            }
        }
        return;
    }
    let segs: Vec<&str> = path.split('.').collect();
    go(doc.as_table_mut(), &segs);
}

/// A line diff of the two texts: the lines that differ between their common head and tail.
pub fn diff(old: &str, new: &str) -> String {
    let (o, n): (Vec<&str>, Vec<&str>) = (old.lines().collect(), new.lines().collect());
    let head = o.iter().zip(&n).take_while(|(a, b)| a == b).count();
    let tail = o[head..]
        .iter()
        .rev()
        .zip(n[head..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let mut out = String::new();
    for l in &o[head..o.len() - tail] {
        out.push_str(&format!("- {l}\n"));
    }
    for l in &n[head..n.len() - tail] {
        out.push_str(&format!("+ {l}\n"));
    }
    out
}

/// What a `set` request does to the file text; the keys it touched are checked for who may
/// write them before anything is edited.
pub fn apply_set(
    d: &Daemon,
    c: &Caller,
    doc: &mut DocumentMut,
    set: &Map<String, Value>,
) -> Result<Vec<&'static K>, Refused> {
    let mut touched = vec![];
    for (path, v) in set {
        let key = find(path).ok_or_else(|| bad(format!("the catalogue has no key `{path}`")))?;
        if key.sens == Fixed {
            return Err((
                409,
                format!(
                    "`{path}` is fixed when masond starts: edit the file and restart the daemon"
                ),
            ));
        }
        if let Some(why) = why_not(d, c, key) {
            return Err((403, format!("`{path}`: {why}")));
        }
        if path.starts_with("provider.") && path != "provider" {
            let segs: Vec<&str> = path.split('.').collect();
            let ok = ["claude", "codex"].contains(&segs[1])
                && mason_definition::tiers::Tier::parse(segs[2]).is_some_and(|_| segs[2] != "fast")
                && mason_definition::tiers::Level::parse(segs[3]).is_some();
            if !ok {
                return Err(bad(format!("`{path}`: use provider.<claude|codex>.<deep|light>.<low|medium|high>.<model|effort>")));
            }
        }
        if path.starts_with("agent_env.") || path.starts_with("remote_runs.test_env.") {
            let name = path.rsplit('.').next().unwrap_or_default();
            if secret_name(name) && !v.is_null() {
                return Err(bad(format!(
                    "`{name}` looks like a secret: the page never accepts one; the agent reads secrets from its own environment"
                )));
            }
        }
        if let Some(step_attr) = path.strip_prefix("step.") {
            let attr = step_attr.rsplit('.').next().unwrap_or_default();
            if let Some(s) = v.as_str().or(None) {
                mason_definition::layers::check_value(attr, s).map_err(bad)?;
            } else if let Some(n) = v.as_i64() {
                mason_definition::layers::check_value(attr, &n.to_string()).map_err(bad)?;
            }
        }
        if v.is_null() {
            unset_at(doc, path);
        } else {
            set_at(doc, path, to_item(key, path, v)?)?;
        }
        touched.push(key);
    }
    Ok(touched)
}

/// After the edit: the loading parsers, the overlay's own checks and the project-file mapping
/// tables must all accept the new text.
pub fn check_text(d: &Daemon, name: &str, new: &str) -> Result<(), Refused> {
    let cfg = crate::config::parse_project(name, new).map_err(bad)?;
    mason_definition::layers::parse_overlay(new).map_err(|e| {
        bad(e
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join("; "))
    })?;
    let eff = crate::tiermap::effective(&d.cfg.config_dir, cfg.agent().provider, new);
    let mine: Vec<&String> = eff
        .problems
        .iter()
        .filter(|p| p.starts_with("project file"))
        .collect();
    if !mine.is_empty() {
        return Err(bad(mine
            .iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("; ")));
    }
    Ok(())
}

/// Does a touched key belong to the overlay the definitions watcher holds?
pub fn touches_overlay(set: &Map<String, Value>) -> bool {
    set.keys().any(|k| {
        k.starts_with("step.")
            || k == "model"
            || k == "test"
            || k == "build"
            || k == "max_test_rounds"
    })
}
