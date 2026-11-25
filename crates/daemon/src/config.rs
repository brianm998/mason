// SPDX-License-Identifier: GPL-3.0-or-later

//! Daemon settings and per-project configuration.
//!
//! A project is one file in the local overlay,
//! `<config dir>/projects/<id>.toml`. (The versioned `.mason/mason.toml`
//! definition arrives in Stage 7; machine facts like these stay local.)

use std::path::{Path, PathBuf};
use std::time::Duration;
use toml_edit::DocumentMut;

/// Default of the `llm:claude` limit: agent sessions at once (ticket 17).
pub const DEFAULT_LLM_SESSIONS: usize = 10;

#[derive(Clone, Debug)]
pub struct DaemonConfig {
    pub data_dir: PathBuf,
    pub config_dir: PathBuf,
    /// Agent sessions at once: the default of the `llm:claude` limit. It is not a cap
    /// on tasks; `resources.toml` and `mason resources limit` override it.
    pub max_concurrent: usize,
    pub sweep_interval: Duration,
    /// How often open debts are looked at: closed when their suite has been green since, the
    /// rest run when the machine is idle (s393).
    pub debt_interval: Duration,
    /// After a suite was red on the base for a debt, the idle pass does not run it again for
    /// debts until a landing touched a module of the red tests or this long has passed (#774).
    pub debt_red_backoff: Duration,
    /// How long after a step run closes (and its hosts exited) the sweep leaves its host
    /// directories in `h/` before moving them to `t/<task>/hosts/` (default a day;
    /// `MASON_HOST_GRACE_MS` sets it for the exit script).
    pub host_grace: Duration,
    /// How long a step host may sit launching before `launch-frozen` is raised.
    pub launch_frozen: Duration,
    /// How many days moved host directories are kept before they are deleted (default 90;
    /// 0 keeps them for ever). `retention_days` in `[daemon]` of `daemon.toml`.
    pub retention_days: u64,
    /// Where CPU use is read for the compile gate. `None`: `MASON_CPU_FILE`, else the OS.
    pub cpu: Option<std::sync::Arc<dyn crate::cpu::CpuSource>>,
    /// Where free disk is read for the disk floor. `None`: `MASON_DISK_FILE`, else the OS.
    pub disk: Option<std::sync::Arc<dyn crate::disk::DiskSource>>,
    /// How often the volume is read (`MASON_DISK_EVERY_MS` sets it for the exit script).
    pub disk_every: Duration,
    pub heartbeat: Duration,
    /// How often the stall watcher reads a silent tree (`--watch-tick-ms` sets it for tests).
    pub watch_tick: Duration,
    pub host_bin: PathBuf,
    pub mcp_bin: PathBuf,
    /// 0 picks a free loopback port (written to `daemon.json`).
    pub port: u16,
    /// LAN access (Part 16): off unless the owner turns it on.
    pub lan: LanCfg,
    /// `[node]`: this daemon accepts work from other Macs. Read once at start.
    pub node: NodeCfg,
    pub stall_threshold: Duration,
    /// How long a provider hold waits before the session is resumed.
    pub hold_retry: Duration,
    /// Enables `POST /debug/stall`.
    pub debug: bool,
    /// How often the definition watcher looks for a changed base branch or
    /// overlay file.
    pub reload_interval: Duration,
    /// How often the watcher fetches each project's remote to see whether the
    /// base branch (and so its definition) changed. Local changes are seen at
    /// `reload_interval`; an explicit reload always fetches.
    pub fetch_interval: Duration,
    /// Git work counts as stuck when the definitions thread has not finished a pass for this long
    /// (beyond its own interval).
    pub stuck_after: Duration,
    /// Whether this daemon raises the "step host launch frozen" alert. The launches it looks
    /// at are the whole process's (`mason_step_host::launcher::launching`), so in a test
    /// process that hosts many daemons one test's slow launch would show up as an alert in
    /// every other test's daemon; the test support turns it off there.
    pub launch_frozen_alert: bool,
    /// A project's fetches must have been failing this long before `fetch:<project>` is raised.
    pub fetch_alert_after: Duration,
    /// A speech command running longer than this for one sentence is killed.
    pub speech_timeout: Duration,
    /// How often alerts are brought up to date and notifications sent.
    pub attention_interval: Duration,
    /// How long a suite waits before trying again after an environmental
    /// failure (no device, the stack would not come up). Charged to no task.
    pub env_retry: Duration,
    /// Wait out the landing window even when no other task is moving toward landing (the
    /// behaviour before #731). Off in production; tests that file several tasks one after the
    /// other and expect one batch turn it on.
    pub landing_window_always: bool,
    /// How often a task waiting because the base is red the same way looks at whether the base
    /// branch has moved (s246).
    pub base_red_poll: Duration,
    /// Automatic retries of failures outside the code (s247): how long after the failure the
    /// first and the second retry come (a third outside failure in `auto_retry_window` stays
    /// failed), how often the loop looks, and the window the retries are counted in.
    pub auto_retry_delays: Vec<Duration>,
    pub auto_retry_window: Duration,
    pub auto_retry_tick: Duration,
    /// How long an explorer's `submit_close` waits for the owner's veto before it closes the
    /// task: for the owner's own tasks, and for tasks MASON or its agents filed (s254). How
    /// often the waiting closes are looked at.
    pub close_window_owner: Duration,
    pub close_window_agent: Duration,
    pub close_tick: Duration,
    /// This many tasks failing outside the code with the same reason within `burst_window` are
    /// one host-wide event: one alert, and their retries wait for a probe.
    pub burst_min: usize,
    pub burst_window: Duration,
    /// Identities whose answers count (Part 10). Default: the user running the
    /// daemon.
    pub owner_names: Vec<String>,
    /// An owner session with no message for this many hours ends by itself (`[sessions]
    /// idle_end_h` in `daemon.toml`; 0: never).
    pub idle_end_h: u64,
    /// The pipeline health check (`[health]` in `daemon.toml`, s432).
    pub health: HealthCfg,
    /// Where Add Project looks for repositories (`[onboarding] roots` in `daemon.toml`).
    pub onboarding: OnboardingCfg,
    /// The commit of the runtime this process is (set when started from
    /// `runtimes/<sha>/`); `None` when run from a build directory.
    pub runtime_sha: Option<String>,
    /// The arguments to start again with when taking over, port included.
    pub exec_args: Vec<String>,
    /// How long a start must live to be called healthy (and not a crash).
    pub healthy_after: Duration,
    /// Where the stage and exit-script numbers of ad-hoc tasks start (above the planned
    /// stages). Read from `MASON_ADHOC_EXIT_START`; default 40.
    pub adhoc_exit_start: u32,
    /// How often the detector cycle runs (Part 14: daily).
    pub introspect_interval: Duration,
    /// How often the analyst agent is asked (weekly).
    pub analyst_interval: Duration,
    /// How often the daemon looks at what is due among the recurring jobs.
    pub schedule_tick: Duration,
}

/// `[lan]` in `daemon.toml`. Loopback stays the default: nothing listens beyond it unless
/// `enabled = true` (or `masond --lan`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LanCfg {
    pub enabled: bool,
    /// 0 asks the OS for a free port.
    pub port: u16,
    /// Extra names or addresses a phone may reach the daemon at (a Tailscale name, say). They go
    /// in the pairing URL after `<host>.local`; they do not change who may connect.
    pub hosts: Vec<String>,
    /// The addresses to listen on, one listener each. Default `0.0.0.0`, as before.
    pub bind: Vec<std::net::IpAddr>,
}

impl Default for LanCfg {
    fn default() -> Self {
        LanCfg {
            enabled: false,
            port: crate::lan::DEFAULT_LAN_PORT,
            hosts: Vec::new(),
            bind: vec![std::net::IpAddr::from([0, 0, 0, 0])],
        }
    }
}

/// Read the `[lan]` table. Any bad value is an error that names the file and the setting.
pub fn parse_lan(text: &str) -> Result<LanCfg, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    let mut lan = LanCfg::default();
    let Some(item) = doc.get("lan") else {
        return Ok(lan);
    };
    let table = item
        .as_table_like()
        .ok_or("daemon.toml: `lan` must be a table: [lan]")?;
    for (name, v) in table.iter() {
        match name {
            "enabled" => {
                lan.enabled = v
                    .as_bool()
                    .ok_or("daemon.toml: lan.enabled must be true or false")?
            }
            "port" => {
                lan.port = v
                    .as_integer()
                    .and_then(|n| u16::try_from(n).ok())
                    .ok_or("daemon.toml: lan.port must be a whole number from 0 to 65535")?
            }
            "hosts" => {
                let bad = "daemon.toml: lan.hosts must be a list of host names or addresses, e.g. [\"mac.tailnet.ts.net\", \"100.64.0.1\"]";
                let arr = v.as_array().ok_or(bad)?;
                lan.hosts.clear();
                for e in arr.iter() {
                    let h = e.as_str().ok_or(bad)?;
                    let ok = !h.is_empty()
                        && h.len() <= 253
                        && h.chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
                    if !ok {
                        return Err(format!(
                            "daemon.toml: lan.hosts entry {h:?} is not a host name or IPv4 address (letters, digits, `.` and `-` only)"
                        ));
                    }
                    if !lan.hosts.iter().any(|x| x == h) {
                        lan.hosts.push(h.to_string());
                    }
                }
            }
            "bind" => {
                let bad = "daemon.toml: lan.bind must be a non-empty list of IP addresses, e.g. [\"0.0.0.0\"]";
                let arr = v.as_array().ok_or(bad)?;
                let mut out: Vec<std::net::IpAddr> = Vec::new();
                for e in arr.iter() {
                    let a: std::net::IpAddr =
                        e.as_str().and_then(|t| t.parse().ok()).ok_or_else(|| {
                            format!("daemon.toml: lan.bind entry {e} is not an IP address")
                        })?;
                    if !out.contains(&a) {
                        out.push(a);
                    }
                }
                if out.is_empty() {
                    return Err(bad.into());
                }
                lan.bind = out;
            }
            other => return Err(format!("daemon.toml: unknown setting lan.{other}")),
        }
    }
    Ok(lan)
}

pub fn load_lan(config_dir: &std::path::Path) -> Result<LanCfg, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_lan(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LanCfg::default()),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

// ---- nodes: running work on other Macs (docs/multi-machine-nodes-plan.md) ---------------------

/// `[node]` in `daemon.toml`: the whole local configuration of a node. Off unless `serve = true`.
/// The owner's on/off switch is not here: it is `node.toml`, changed live.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeCfg {
    pub serve: bool,
    /// 0 asks the OS for a free port.
    pub port: u16,
    /// The addresses to listen on, one listener each.
    pub bind: Vec<std::net::IpAddr>,
    /// The node protocol this node speaks. Not a setting: a test says another to see what a
    /// coordinator does with a node it cannot work with.
    pub proto: u32,
}

impl Default for NodeCfg {
    fn default() -> Self {
        NodeCfg {
            serve: false,
            port: crate::node_serve::DEFAULT_NODE_PORT,
            bind: vec![std::net::IpAddr::from([0, 0, 0, 0])],
            proto: crate::node_serve::NODE_PROTO,
        }
    }
}

/// Read the `[node]` table. Any bad value is an error that names the file and the setting.
pub fn parse_node(text: &str) -> Result<NodeCfg, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    let mut node = NodeCfg::default();
    let Some(item) = doc.get("node") else {
        return Ok(node);
    };
    let table = item
        .as_table_like()
        .ok_or("daemon.toml: `node` must be a table: [node]")?;
    for (name, v) in table.iter() {
        match name {
            "serve" => {
                node.serve = v
                    .as_bool()
                    .ok_or("daemon.toml: node.serve must be true or false")?
            }
            "port" => {
                node.port = v
                    .as_integer()
                    .and_then(|n| u16::try_from(n).ok())
                    .ok_or("daemon.toml: node.port must be a whole number from 0 to 65535")?
            }
            "bind" => {
                let bad = "daemon.toml: node.bind must be a non-empty list of IP addresses, e.g. [\"0.0.0.0\"]";
                let arr = v.as_array().ok_or(bad)?;
                let mut out: Vec<std::net::IpAddr> = Vec::new();
                for e in arr.iter() {
                    let a: std::net::IpAddr =
                        e.as_str().and_then(|t| t.parse().ok()).ok_or_else(|| {
                            format!("daemon.toml: node.bind entry {e} is not an IP address")
                        })?;
                    if !out.contains(&a) {
                        out.push(a);
                    }
                }
                if out.is_empty() {
                    return Err(bad.into());
                }
                node.bind = out;
            }
            other => return Err(format!("daemon.toml: unknown setting node.{other}")),
        }
    }
    Ok(node)
}

pub fn load_node(config_dir: &std::path::Path) -> Result<NodeCfg, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_node(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(NodeCfg::default()),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

/// What a node does with running jobs when the owner turns the switch off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OnSwitchOff {
    /// Let them finish (the default); no new job starts.
    Drain,
    /// Kill them; the coordinator places them again.
    Cancel,
}

impl OnSwitchOff {
    pub fn name(self) -> &'static str {
        match self {
            OnSwitchOff::Drain => "drain",
            OnSwitchOff::Cancel => "cancel",
        }
    }
}

/// The tunables of a node, set on the coordinator and sent to the node with every status poll.
/// A node that has heard nothing uses [`NodeSettings::default`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeSettings {
    pub max_jobs: u32,
    /// The node takes no job while other work uses more of its CPU than this.
    pub max_other_cpu_pct: u32,
    pub min_disk_free_gb: u64,
    pub on_switch_off: OnSwitchOff,
    /// A job whose coordinator has not asked after it for this long is cancelled.
    pub lease_s: u64,
    /// 0: the node's own thermal defaults.
    pub max_die_c: u32,
}

impl Default for NodeSettings {
    fn default() -> Self {
        NodeSettings {
            max_jobs: 2,
            max_other_cpu_pct: 50,
            min_disk_free_gb: 50,
            on_switch_off: OnSwitchOff::Drain,
            lease_s: 300,
            max_die_c: 0,
        }
    }
}

/// Some of a node's settings: `[nodes.default]` or `[nodes.<name>]`, laid over the built-ins.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SettingsPatch {
    pub max_jobs: Option<u32>,
    pub max_other_cpu_pct: Option<u32>,
    pub min_disk_free_gb: Option<u64>,
    pub on_switch_off: Option<OnSwitchOff>,
    pub lease_s: Option<u64>,
    pub max_die_c: Option<u32>,
}

impl SettingsPatch {
    /// One setting, checked: `what` names it in the error ("nodes.default.max_jobs").
    pub fn set(&mut self, key: &str, v: &toml_edit::Item, what: &str) -> Result<(), String> {
        let int = |lo: i64, hi: i64| -> Result<i64, String> {
            v.as_integer()
                .filter(|n| (lo..=hi).contains(n))
                .ok_or_else(|| {
                    format!("daemon.toml: {what} must be a whole number from {lo} to {hi}")
                })
        };
        match key {
            "max_jobs" => self.max_jobs = Some(int(1, 64)? as u32),
            "max_other_cpu_pct" => self.max_other_cpu_pct = Some(int(1, 100)? as u32),
            "min_disk_free_gb" => self.min_disk_free_gb = Some(int(0, 100_000)? as u64),
            "lease_s" => self.lease_s = Some(int(10, 3600)? as u64),
            "max_die_c" => {
                let n = int(0, 110)?;
                if n != 0 && n < 50 {
                    return Err(format!(
                        "daemon.toml: {what} must be 0 (the node's own default) or from 50 to 110"
                    ));
                }
                self.max_die_c = Some(n as u32)
            }
            "on_switch_off" => {
                self.on_switch_off = Some(match v.as_str() {
                    Some("drain") => OnSwitchOff::Drain,
                    Some("cancel") => OnSwitchOff::Cancel,
                    _ => {
                        return Err(format!(
                            "daemon.toml: {what} must be \"drain\" or \"cancel\""
                        ))
                    }
                })
            }
            other => {
                let (table, _) = what.rsplit_once('.').unwrap_or((what, ""));
                return Err(format!("daemon.toml: unknown setting {table}.{other}"));
            }
        }
        Ok(())
    }

    pub fn apply(&self, s: &mut NodeSettings) {
        if let Some(v) = self.max_jobs {
            s.max_jobs = v;
        }
        if let Some(v) = self.max_other_cpu_pct {
            s.max_other_cpu_pct = v;
        }
        if let Some(v) = self.min_disk_free_gb {
            s.min_disk_free_gb = v;
        }
        if let Some(v) = self.on_switch_off {
            s.on_switch_off = v;
        }
        if let Some(v) = self.lease_s {
            s.lease_s = v;
        }
        if let Some(v) = self.max_die_c {
            s.max_die_c = v;
        }
    }
}

impl NodeSettings {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "max_jobs": self.max_jobs,
            "max_other_cpu_pct": self.max_other_cpu_pct,
            "min_disk_free_gb": self.min_disk_free_gb,
            "on_switch_off": self.on_switch_off.name(),
            "lease_s": self.lease_s,
            "max_die_c": self.max_die_c,
        })
    }

    /// Settings as a coordinator sent them. A missing key keeps the built-in value; a key of the
    /// wrong kind or out of range is an error that names it.
    pub fn from_json(v: &serde_json::Value) -> Result<NodeSettings, String> {
        let mut patch = SettingsPatch::default();
        let Some(o) = v.as_object() else {
            return Err("the settings must be a JSON object".into());
        };
        for (k, x) in o {
            let item = match x {
                serde_json::Value::String(t) => toml_edit::value(t.as_str()),
                serde_json::Value::Number(n) if n.is_i64() || n.is_u64() => {
                    toml_edit::value(i64::try_from(n.as_u64().unwrap_or(0)).unwrap_or(i64::MAX))
                }
                _ => return Err(format!("setting {k} has the wrong kind of value")),
            };
            patch
                .set(k, &item, &format!("settings.{k}"))
                .map_err(|e| e.trim_start_matches("daemon.toml: ").to_string())?;
        }
        let mut s = NodeSettings::default();
        patch.apply(&mut s);
        Ok(s)
    }

    /// What applies when several coordinators have sent settings: the strictest of each. Fewer
    /// jobs, less CPU the node's own work may leave to others, more disk kept free, the shorter
    /// lease, the lower die limit (0 means "no opinion"), and `cancel` if any coordinator wants it.
    pub fn strictest(all: &[NodeSettings]) -> NodeSettings {
        let mut it = all.iter();
        let Some(first) = it.next() else {
            return NodeSettings::default();
        };
        it.fold(*first, |a, b| NodeSettings {
            max_jobs: a.max_jobs.min(b.max_jobs),
            max_other_cpu_pct: a.max_other_cpu_pct.min(b.max_other_cpu_pct),
            min_disk_free_gb: a.min_disk_free_gb.max(b.min_disk_free_gb),
            on_switch_off: if a.on_switch_off == OnSwitchOff::Cancel
                || b.on_switch_off == OnSwitchOff::Cancel
            {
                OnSwitchOff::Cancel
            } else {
                OnSwitchOff::Drain
            },
            lease_s: a.lease_s.min(b.lease_s),
            max_die_c: match (a.max_die_c, b.max_die_c) {
                (0, x) | (x, 0) => x,
                (x, y) => x.min(y),
            },
        })
    }
}

/// Which off-machine target is tried first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Prefer {
    Nodes,
    Github,
}

/// `[nodes]` in `daemon.toml`: the coordinator's side. Read live (`load_nodes`), so a change
/// needs no restart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodesCfg {
    pub enabled: bool,
    pub prefer: Prefer,
    /// `host:port` of nodes mDNS cannot see.
    pub static_hosts: Vec<String>,
    pub status_every_s: u64,
    /// Failed polls in a row after which a node counts as gone.
    pub lost_after_polls: u32,
    /// Only these projects may use nodes; `None`: every project.
    pub projects: Option<Vec<String>>,
    pub exclude: Vec<String>,
    /// Send each paired node the runtime this coordinator runs, cross-built and signed for the
    /// node's architecture, when the node is on another (`push_runtime`, default true; §3.14).
    /// It does nothing without a `sign_identity` in `update.toml`.
    pub push_runtime: bool,
    pub default: SettingsPatch,
    pub per_node: std::collections::BTreeMap<String, SettingsPatch>,
}

impl Default for NodesCfg {
    fn default() -> Self {
        NodesCfg {
            enabled: false,
            prefer: Prefer::Nodes,
            static_hosts: Vec::new(),
            status_every_s: 5,
            lost_after_polls: 6,
            projects: None,
            exclude: Vec::new(),
            push_runtime: true,
            default: SettingsPatch::default(),
            per_node: std::collections::BTreeMap::new(),
        }
    }
}

impl NodesCfg {
    /// The settings sent to the node called `name`: the built-ins, then `[nodes.default]`, then
    /// `[nodes.<name>]`.
    pub fn settings_for(&self, name: &str) -> NodeSettings {
        let mut s = NodeSettings::default();
        self.default.apply(&mut s);
        if let Some(p) = self.per_node.get(name) {
            p.apply(&mut s);
        }
        s
    }

    /// May `project` use nodes?
    pub fn allows(&self, project: &str) -> bool {
        self.projects
            .as_ref()
            .is_none_or(|p| p.iter().any(|x| x == project))
            && !self.exclude.iter().any(|x| x == project)
    }
}

/// A `host:port` that is a host name or address and a port.
pub fn valid_host_port(h: &str) -> bool {
    let Some((host, port)) = h.rsplit_once(':') else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    !host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | ':'))
        && port.parse::<u16>().is_ok_and(|p| p > 0)
}

/// Read the `[nodes]` tables. Any bad value is an error that names the file and the setting.
pub fn parse_nodes(text: &str) -> Result<NodesCfg, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    let mut cfg = NodesCfg::default();
    let Some(item) = doc.get("nodes") else {
        return Ok(cfg);
    };
    let table = item
        .as_table_like()
        .ok_or("daemon.toml: `nodes` must be a table: [nodes]")?;
    let names = |v: &toml_edit::Item, what: &str| -> Result<Vec<String>, String> {
        let bad = format!("daemon.toml: {what} must be a list of project names");
        let arr = v.as_array().ok_or(bad.clone())?;
        arr.iter()
            .map(|e| e.as_str().map(String::from).ok_or(bad.clone()))
            .collect()
    };
    for (name, v) in table.iter() {
        match name {
            "enabled" => {
                cfg.enabled = v
                    .as_bool()
                    .ok_or("daemon.toml: nodes.enabled must be true or false")?
            }
            "prefer" => {
                cfg.prefer = match v.as_str() {
                    Some("nodes") => Prefer::Nodes,
                    Some("github") => Prefer::Github,
                    _ => {
                        return Err(
                            "daemon.toml: nodes.prefer must be \"nodes\" or \"github\"".into()
                        )
                    }
                }
            }
            "static" => {
                let bad = "daemon.toml: nodes.static must be a list of host:port, e.g. [\"studio.local:7425\"]";
                let arr = v.as_array().ok_or(bad)?;
                for e in arr.iter() {
                    let h = e.as_str().filter(|h| valid_host_port(h)).ok_or_else(|| {
                        format!("daemon.toml: nodes.static entry {e} is not a host:port")
                    })?;
                    if !cfg.static_hosts.iter().any(|x| x == h) {
                        cfg.static_hosts.push(h.to_string());
                    }
                }
            }
            "status_every_s" => {
                cfg.status_every_s = v.as_integer().filter(|n| (1..=3600).contains(n)).ok_or(
                    "daemon.toml: nodes.status_every_s must be a whole number from 1 to 3600",
                )? as u64
            }
            "lost_after_polls" => {
                cfg.lost_after_polls = v.as_integer().filter(|n| (1..=1000).contains(n)).ok_or(
                    "daemon.toml: nodes.lost_after_polls must be a whole number from 1 to 1000",
                )? as u32
            }
            "projects" => cfg.projects = Some(names(v, "nodes.projects")?),
            "exclude" => cfg.exclude = names(v, "nodes.exclude")?,
            "push_runtime" => {
                cfg.push_runtime = v
                    .as_bool()
                    .ok_or("daemon.toml: nodes.push_runtime must be true or false")?
            }
            sub => {
                let t = v.as_table_like().ok_or_else(|| {
                    format!("daemon.toml: unknown setting nodes.{sub}; a node's settings are a table: [nodes.{sub}]")
                })?;
                let mut patch = SettingsPatch::default();
                for (k, x) in t.iter() {
                    patch.set(k, x, &format!("nodes.{sub}.{k}"))?;
                }
                if sub == "default" {
                    cfg.default = patch;
                } else {
                    cfg.per_node.insert(sub.to_string(), patch);
                }
            }
        }
    }
    Ok(cfg)
}

pub fn load_nodes(config_dir: &std::path::Path) -> Result<NodesCfg, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_nodes(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(NodesCfg::default()),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

/// `[targets]` in `daemon.toml`: where devices come from.
///
/// ```toml
/// [targets]
/// provider = "system"   # or "fake", which reads fake_file (tests)
/// fake_file = "/path/devices.json"
/// fake_log = "/path/calls.log"   # optional
/// poll_ms = 2000
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TargetsCfg {
    pub fake: bool,
    pub fake_file: Option<PathBuf>,
    pub fake_log: Option<PathBuf>,
    pub poll_ms: u64,
}

impl Default for TargetsCfg {
    fn default() -> Self {
        TargetsCfg {
            fake: false,
            fake_file: None,
            fake_log: None,
            poll_ms: 2000,
        }
    }
}

pub fn parse_targets(text: &str) -> Result<TargetsCfg, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    let mut t = TargetsCfg::default();
    let Some(item) = doc.get("targets") else {
        return Ok(t);
    };
    let table = item
        .as_table_like()
        .ok_or("daemon.toml: `targets` must be a table: [targets]")?;
    for (name, v) in table.iter() {
        let text = |what: &str| {
            v.as_str()
                .map(String::from)
                .ok_or_else(|| format!("daemon.toml: targets.{what} must be a string"))
        };
        match name {
            "provider" => match text("provider")?.as_str() {
                "system" => t.fake = false,
                "fake" => t.fake = true,
                other => {
                    return Err(format!(
                    "daemon.toml: targets.provider must be \"system\" or \"fake\", not \"{other}\""
                ))
                }
            },
            "fake_file" => t.fake_file = Some(PathBuf::from(text("fake_file")?)),
            "fake_log" => t.fake_log = Some(PathBuf::from(text("fake_log")?)),
            "poll_ms" => {
                t.poll_ms = v
                    .as_integer()
                    .and_then(|n| u64::try_from(n).ok())
                    .filter(|n| *n >= 50)
                    .ok_or("daemon.toml: targets.poll_ms must be a whole number of at least 50")?
            }
            other => return Err(format!("daemon.toml: unknown setting targets.{other}")),
        }
    }
    if t.fake && t.fake_file.is_none() {
        return Err("daemon.toml: targets.provider = \"fake\" needs targets.fake_file".into());
    }
    Ok(t)
}

pub fn load_targets(config_dir: &std::path::Path) -> Result<TargetsCfg, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_targets(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(TargetsCfg::default()),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

/// The loopback port `masond` listens on when neither `--port` nor `[daemon] port`
/// says otherwise. Fixed, so a running monitor finds the daemon again after a
/// restart. (7421, first proposed, is assigned in /etc/services.)
pub const DEFAULT_PORT: u16 = 7423;

/// Which port to bind: `--port`, then `[daemon] port` in `daemon.toml`, then the
/// default. A demo keeps nothing and runs beside the real daemon, so it takes a
/// free port (0) unless told otherwise.
pub fn resolve_port(cli: Option<u16>, file: Option<u16>, demo: bool) -> u16 {
    cli.or(file).unwrap_or(if demo { 0 } else { DEFAULT_PORT })
}

/// `daemon.toml` in the config directory:
///
/// ```toml
/// [daemon]
/// port = 7423   # 0 asks the OS for a free port each start
/// ```
/// How long moved host directories are kept, in days, unless `daemon.toml` says otherwise.
pub const DEFAULT_RETENTION_DAYS: u64 = 90;

/// `retention_days` from `[daemon]` in `daemon.toml`, if set.
pub fn parse_retention_days(text: &str) -> Result<Option<u64>, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    match doc.get("daemon").and_then(|d| d.get("retention_days")) {
        None => Ok(None),
        Some(v) => v
            .as_integer()
            .and_then(|n| u64::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| {
                "daemon.toml: daemon.retention_days must be a whole number of days (0 keeps them for ever)"
                    .to_string()
            }),
    }
}

pub fn load_retention_days(config_dir: &std::path::Path) -> Result<Option<u64>, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_retention_days(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

/// Hours without a message after which an owner session ends by itself, unless `daemon.toml`
/// says otherwise.
pub const DEFAULT_IDLE_END_H: u64 = 24;

/// `idle_end_h` from `[sessions]` in `daemon.toml`, if set.
pub fn parse_idle_end_h(text: &str) -> Result<Option<u64>, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    match doc.get("sessions").and_then(|d| d.get("idle_end_h")) {
        None => Ok(None),
        Some(v) => v
            .as_integer()
            .and_then(|n| u64::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| {
                "daemon.toml: sessions.idle_end_h must be a whole number of hours (0: never)"
                    .to_string()
            }),
    }
}

/// `[onboarding]` in `daemon.toml`: the directories whose children Add Project offers.
///
/// ```toml
/// [onboarding]
/// roots = ["~/git"]
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OnboardingCfg {
    /// Each root with `~` already expanded to the home directory.
    pub roots: Vec<PathBuf>,
}

impl Default for OnboardingCfg {
    fn default() -> Self {
        OnboardingCfg {
            roots: vec![expand_home("~/git", &home_dir())],
        }
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// `~` and `~/x` mean the home directory; anything else is taken as written.
pub fn expand_home(p: &str, home: &Path) -> PathBuf {
    if p == "~" {
        home.to_path_buf()
    } else if let Some(rest) = p.strip_prefix("~/") {
        home.join(rest)
    } else {
        PathBuf::from(p)
    }
}

/// `[onboarding]`: `roots`, a list of directories (`~` expanded against `home`).
pub fn parse_onboarding(text: &str, home: &Path) -> Result<OnboardingCfg, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    let mut c = OnboardingCfg {
        roots: vec![expand_home("~/git", home)],
    };
    let Some(t) = doc.get("onboarding") else {
        return Ok(c);
    };
    let t = t
        .as_table_like()
        .ok_or("daemon.toml: [onboarding] must be a table")?;
    for (k, v) in t.iter() {
        match k {
            "roots" => {
                let bad =
                    "daemon.toml: onboarding.roots must be a list of directories, e.g. [\"~/git\"]";
                let arr = v.as_array().ok_or(bad)?;
                c.roots.clear();
                for e in arr.iter() {
                    let r = e.as_str().filter(|r| !r.trim().is_empty()).ok_or(bad)?;
                    let r = expand_home(r.trim(), home);
                    if !c.roots.contains(&r) {
                        c.roots.push(r);
                    }
                }
            }
            other => return Err(format!("daemon.toml: unknown setting onboarding.{other}")),
        }
    }
    Ok(c)
}

pub fn load_onboarding(config_dir: &Path) -> Result<OnboardingCfg, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_onboarding(&t, &home_dir()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(OnboardingCfg::default()),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

/// `[health]` in `daemon.toml`: how often the pipeline checks itself and its thresholds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HealthCfg {
    /// Minutes between checks; 0 turns the loop off (`mason health` still works).
    pub interval_m: u64,
    pub limits: mason_core::health::Limits,
}

impl Default for HealthCfg {
    fn default() -> HealthCfg {
        HealthCfg {
            interval_m: 30,
            limits: Default::default(),
        }
    }
}

/// `[health]`: `interval_m` and the thresholds, each a whole number in the unit its name says.
pub fn parse_health(text: &str) -> Result<Option<HealthCfg>, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    let Some(t) = doc.get("health") else {
        return Ok(None);
    };
    let t = t
        .as_table_like()
        .ok_or("daemon.toml: [health] must be a table")?;
    let mut c = HealthCfg::default();
    let l = &mut c.limits;
    const M: u64 = 60_000;
    const H: u64 = 3_600_000;
    for (k, v) in t.iter() {
        let n = v
            .as_integer()
            .and_then(|n| u64::try_from(n).ok())
            .ok_or_else(|| format!("daemon.toml: health.{k} must be a whole number"))?;
        match k {
            "interval_m" => c.interval_m = n,
            "stuck_factor" => l.stuck_factor = n,
            "stuck_min_m" => l.stuck_min_ms = n * M,
            "wait_m" => l.wait_ms = n * M,
            "aged_wait_h" => l.aged_wait_ms = n * H,
            "queue_idle_m" => l.queue_idle_ms = n * M,
            "host_silent_m" => l.host_silent_ms = n * M,
            "slot_wait_m" => l.slot_wait_ms = n * M,
            "load_pct" => l.load_pct = n as u32,
            "stall_ms" => l.stall_ms = n,
            "frozen_launch_s" => l.frozen_launch_ms = n * 1000,
            "mem_free_mb" => l.mem_free_mb = n,
            "cpu_gate_m" => l.cpu_gate_ms = n * M,
            "undiagnosed_m" => l.undiagnosed_ms = n * M,
            "pending_diagnosis_h" => l.pending_diagnosis_ms = n * H,
            "same_failure" => l.same_failure = n as usize,
            "red_h" => l.red_ms = n * H,
            "update_failures" => l.update_failures = n as u32,
            "restarts" => l.restarts = n as u32,
            "late_loops" => l.late_loops = n as u32,
            "cannot_land" => l.cannot_land = n as u32,
            "cooldown_h" => l.cooldown_ms = n * H,
            "blocked_compiles" => l.blocked_compiles = n as u32,
            "blocked_compile_m" => l.blocked_compile_ms = n * M,
            other => return Err(format!("daemon.toml: health.{other} is not a known key")),
        }
    }
    Ok(Some(c))
}

pub fn load_health(config_dir: &std::path::Path) -> Result<Option<HealthCfg>, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_health(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

pub fn load_idle_end_h(config_dir: &std::path::Path) -> Result<Option<u64>, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_idle_end_h(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

pub fn parse_daemon_port(text: &str) -> Result<Option<u16>, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    let mut port = None;
    for (key, item) in doc.iter() {
        if matches!(
            key,
            "lan" | "targets" | "sessions" | "onboarding" | "node" | "nodes"
        ) {
            continue; // read by `parse_lan`, `parse_targets`, `parse_node` and `parse_nodes`
        }
        let table = item
            .as_table_like()
            .filter(|_| key == "daemon")
            .ok_or_else(|| format!("daemon.toml: unknown setting `{key}`"))?;
        for (name, v) in table.iter() {
            match name {
                "port" => {
                    port = Some(
                        v.as_integer()
                            .and_then(|n| u16::try_from(n).ok())
                            .ok_or_else(|| {
                                "daemon.toml: daemon.port must be a whole number from 0 to 65535"
                                    .to_string()
                            })?,
                    )
                }
                "retention_days" => {} // read by `parse_retention_days`
                other => return Err(format!("daemon.toml: unknown setting daemon.{other}")),
            }
        }
    }
    Ok(port)
}

pub fn load_daemon_port(config_dir: &std::path::Path) -> Result<Option<u16>, String> {
    match std::fs::read_to_string(config_dir.join("daemon.toml")) {
        Ok(t) => parse_daemon_port(&t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("daemon.toml: {e}")),
    }
}

impl DaemonConfig {
    pub fn new(
        data_dir: impl Into<PathBuf>,
        config_dir: impl Into<PathBuf>,
        host_bin: PathBuf,
        mcp_bin: PathBuf,
    ) -> Self {
        DaemonConfig {
            data_dir: data_dir.into(),
            config_dir: config_dir.into(),
            max_concurrent: DEFAULT_LLM_SESSIONS,
            cpu: None,
            disk: None,
            disk_every: crate::disk::DEFAULT_EVERY,
            sweep_interval: Duration::from_secs(120),
            debt_interval: Duration::from_secs(3600),
            debt_red_backoff: Duration::from_secs(24 * 3600),
            host_grace: std::env::var("MASON_HOST_GRACE_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map_or(Duration::from_secs(24 * 3600), Duration::from_millis),
            launch_frozen: std::env::var("MASON_LAUNCH_FROZEN_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map_or(
                    Duration::from_millis(crate::attention::LAUNCH_FROZEN_MS),
                    Duration::from_millis,
                ),
            retention_days: DEFAULT_RETENTION_DAYS,
            heartbeat: Duration::from_secs(1),
            watch_tick: Duration::from_secs(5),
            host_bin,
            mcp_bin,
            port: 0,
            lan: LanCfg::default(),
            node: NodeCfg::default(),
            stall_threshold: Duration::from_millis(500),
            hold_retry: Duration::from_secs(60),
            debug: false,
            reload_interval: Duration::from_secs(5),
            fetch_interval: Duration::from_secs(60),
            stuck_after: Duration::from_secs(300),
            launch_frozen_alert: true,
            fetch_alert_after: Duration::from_secs(15 * 60),
            speech_timeout: Duration::from_secs(30),
            attention_interval: Duration::from_secs(5),
            env_retry: Duration::from_secs(30),
            landing_window_always: false,
            base_red_poll: Duration::from_secs(60),
            auto_retry_delays: vec![Duration::from_secs(120), Duration::from_secs(600)],
            auto_retry_window: Duration::from_secs(24 * 3600),
            auto_retry_tick: Duration::from_secs(30),
            close_window_owner: Duration::from_secs(24 * 3600),
            close_window_agent: Duration::from_secs(2 * 3600),
            close_tick: Duration::from_secs(30),
            burst_min: 3,
            burst_window: Duration::from_secs(600),
            runtime_sha: None,
            exec_args: vec![],
            healthy_after: Duration::from_secs(30),
            adhoc_exit_start: std::env::var("MASON_ADHOC_EXIT_START")
                .ok()
                .and_then(|v| v.parse().ok())
                .filter(|n| (1..=crate::numbers::EXIT_CEILING).contains(n))
                .unwrap_or(crate::numbers::DEFAULT_ADHOC_START),
            introspect_interval: Duration::from_secs(24 * 3600),
            analyst_interval: Duration::from_secs(7 * 24 * 3600),
            schedule_tick: Duration::from_secs(30),
            owner_names: vec![std::env::var("USER")
                .or_else(|_| std::env::var("LOGNAME"))
                .unwrap_or_else(|_| "owner".into())],
            idle_end_h: DEFAULT_IDLE_END_H,
            health: HealthCfg::default(),
            onboarding: OnboardingCfg::default(),
        }
    }

    pub fn store_path(&self) -> PathBuf {
        self.data_dir.join("store.db")
    }
    /// Step hosts. Short on purpose: each host's socket lives under it.
    pub fn hosts_dir(&self) -> PathBuf {
        self.data_dir.join("h")
    }
    /// Contract sockets, also short.
    pub fn sockets_dir(&self) -> PathBuf {
        self.data_dir.join("s")
    }
    pub fn worktrees_dir(&self) -> PathBuf {
        self.data_dir.join("w")
    }
    pub fn landing_dir(&self) -> PathBuf {
        self.data_dir.join("l")
    }
    /// The cache directory MASON owns for `task` (`MASON_TASK_CACHE`): removed when the task
    /// lands or is canceled.
    pub fn task_cache_dir(&self, task: u64) -> PathBuf {
        self.data_dir.join("c").join(task.to_string())
    }
    pub fn task_dir(&self, task: u64) -> PathBuf {
        self.data_dir.join("t").join(task.to_string())
    }
    /// Where the host directories of closed step runs of `task` are kept.
    pub fn archived_hosts_dir(&self, task: u64) -> PathBuf {
        self.task_dir(task).join("hosts")
    }
    pub fn agent_state_dir(&self) -> PathBuf {
        self.data_dir.join("a")
    }
    pub fn info_path(&self) -> PathBuf {
        self.data_dir.join("daemon.json")
    }
}

#[derive(Clone, Debug)]
pub struct ProjectCfg {
    pub id: String,
    pub path: PathBuf,
    pub base: String,
    pub remote: Option<String>,
    pub test_cmd: String,
    pub claude_bin: Option<PathBuf>,
    pub model: Option<String>,
    pub agent_env: Vec<(String, String)>,
    pub tool_timeout: Duration,
    pub step_timeout: Duration,
    /// An agent that has been silent this long raises an alert (`agent_idle_alert_s`,
    /// default 2700); zero is off.
    pub agent_idle_alert: Duration,
    /// ... and this long is stopped and continued (`agent_idle_stop_s`, default 5400); zero is off.
    pub agent_idle_stop: Duration,
    /// A suite or command whose process tree has been silent (no output, no CPU) this long
    /// has its stopped processes continued (`cont_after_s`, default 0 = off).
    pub cont_after: Duration,
    /// ... and one silent this long with nothing waiting on a lease is killed and judged red
    /// (`kill_after_s`, default 0 = off).
    pub kill_after: Duration,
    pub test_timeout: Duration,
    /// Run a diagnosis session for a task that fails on its own code (`diagnose`, default true;
    /// s250).
    pub diagnose: bool,
    /// The model of the diagnosis session (`diagnose_model`), if the registration names one.
    pub diagnose_model: Option<String>,
    /// Its effort (`diagnose_effort`), one of the provider's efforts.
    pub diagnose_effort: Option<String>,
    /// Retry a failure outside the code by itself (`auto_retry`, default true; s247).
    pub auto_retry: bool,
    /// Test rounds before the task fails, if the overlay sets one (layer 3).
    pub max_test_rounds: Option<u32>,
    /// How many times a red test command is run again before the round counts as
    /// red (`test_reruns`, default 0). Green on a rerun is green, recorded as a
    /// flake; red every time is red. For a suite of many tests that only some
    /// machines' load makes fail, this judges the change instead of the machine.
    pub test_reruns: u32,
    /// How a rerun runs only the tests that failed (`rerun_filter`, optional): `"cargo"`
    /// appends `-- --exact <names>`; a template with `{tests}` has it replaced by the quoted
    /// names. Without it a rerun runs the whole command (the names are still compared).
    pub rerun_filter: Option<String>,
    /// Where a task's build directory is, e.g. `$HOME/.cache/mason-target/{task}` (`task_cache`,
    /// optional). Removed when the task lands (s78). Without it nothing is removed.
    pub task_cache: Option<String>,
    /// Days a canceled or failed task's `task_cache` directory is kept after its last step
    /// (`task_cache_keep_days`; default 7).
    pub task_cache_keep_days: Option<u64>,
    /// Clone the landing area's warm build into a new task's directory when it is of the commit
    /// the task starts from (`task_cache_seed`; default off, decision 0552).
    pub task_cache_seed: bool,
    /// A command that reads a task body on stdin and prints `clear` or
    /// `unclear`. Optional, and only consulted when the deterministic check is
    /// unsure. Anything but `clear`, or a failure, means explore.
    pub clarity_classifier: Option<String>,
    /// Opt in to letting top-priority tasks jump the "fewest steps remaining"
    /// term of the scheduling rank (Part 7). Off by default.
    pub priority_jump: bool,
    /// Seconds a queued task is held back for an in-flight task that changes the same files
    /// (`hold_back_secs`, default 7200; 0 turns the hold off) (s398).
    pub hold_back_secs: u64,
    /// How many tasks beyond the LLM limit may be in agent steps at once while their agents are
    /// parked on checks (`park_extra`; default: the limit) (#765, decision 0749).
    pub park_extra: Option<u32>,
    /// A narrowed test-step suite whose selected targets' recorded run times sum to less than
    /// this many seconds takes the `test-small` lane (`small_run_s`, default 180; 0 turns it
    /// off) (s649).
    pub small_run_s: u64,
    /// A landing batch leaves out the cargo test targets that are green on the same inputs
    /// (`reuse_green`, default off) (s707, decision 0700).
    pub reuse_green: bool,
    /// With `reuse_green`, a batch runs every target and checks the recorded keys when none
    /// did in this many hours (`reuse_full_hours`, default 24) (s707).
    pub reuse_full_hours: u64,
    /// Globs of shared files that are expected to change often; they never hold a task back
    /// (`hold_back_ignore`) (s398).
    pub hold_back_ignore: Vec<String>,
    /// A cheap command that rewrites a decision card that does not fit: it reads
    /// `{"card": ..., "problems": [...]}` on stdin and prints a card as JSON.
    /// Optional; without it (or if it fails) a deterministic fallback is used.
    pub card_rewriter: Option<String>,
    /// The overlay file's text, for the configuration layers.
    pub overlay_text: String,
    /// The outside services: git host, build system, issue tracker.
    pub services: ServicesCfg,
    /// Present when this project is MASON's own repository (`[self]`): its changes
    /// to gates and the like stop for the owner, and its self-tasks are limited.
    pub self_cfg: Option<SelfCfg>,
    /// The provider account this project's agent steps use (`account` in the
    /// overlay). Filled for real by `load_projects`, which knows the accounts.
    pub account: String,
    /// The machine's config dir (where approvals are kept), filled by `load_project(s)`;
    /// empty for a project parsed from text. A `plugin:<name>` service needs it.
    pub config_dir: PathBuf,
}

/// Which agent CLI runs a project's agent steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Provider {
    #[default]
    Claude,
    Codex,
}

/// The agent keys of the project file: provider, the retry ladder and the
/// sandbox. Read from the overlay text, so the definition, the layers and the
/// validator need to know nothing of them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentCfg {
    pub provider: Provider,
    /// `account` in the overlay: a name from providers.toml.
    pub account: Option<String>,
    pub codex_bin: Option<PathBuf>,
    /// Models weakest to strongest. A step that ends with no result, or crashes,
    /// is tried again from a fresh session on the next model up. Empty: no change.
    pub ladder: Vec<String>,
    /// Retries up the ladder per step.
    pub max_retries: u32,
    pub sandbox: mason_adapters::sandbox::SandboxKind,
    /// The model the last round a task may get runs on (`escalate_model`; default `opus` for
    /// the Claude provider, none for the others; empty turns it off).
    pub escalate_model: Option<String>,
}

pub fn parse_agent(id: &str, doc: &DocumentMut) -> Result<AgentCfg, String> {
    let s = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(String::from);
    // `provider = "codex"`, or, when the file also holds `[provider.claude]`
    // mapping tables (TOML cannot hold both a string and a table under one
    // name), `[provider]` with `use = "codex"`. See `providers.rs`.
    let chosen = s("provider").or_else(|| {
        doc.get("provider")
            .and_then(|v| v.as_table_like())
            .and_then(|t| t.get("use"))
            .and_then(|v| v.as_str())
            .map(String::from)
    });
    let provider = match chosen.as_deref() {
        None | Some("claude") => Provider::Claude,
        Some("codex") => Provider::Codex,
        Some(o) => {
            return Err(format!(
                "{id}: `provider` is `claude` or `codex`, not {o:?}"
            ))
        }
    };
    let account = match doc.get("account") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .filter(|a| valid_id(a))
                .ok_or_else(|| format!("{id}: `account` is the name of a providers.toml account"))?
                .to_string(),
        ),
    };
    let ladder: Vec<String> = match doc.get("ladder") {
        None => vec![],
        Some(v) => v
            .as_array()
            .ok_or_else(|| format!("{id}: `ladder` must be a list of model names"))?
            .iter()
            .map(|x| {
                x.as_str()
                    .filter(|m| !m.trim().is_empty())
                    .map(String::from)
                    .ok_or_else(|| format!("{id}: `ladder` holds model names (strings)"))
            })
            .collect::<Result<_, _>>()?,
    };
    let escalate_model = match doc.get("escalate_model") {
        None => (provider == Provider::Claude).then(|| "opus".to_string()),
        Some(v) => Some(
            v.as_str()
                .ok_or_else(|| format!("{id}: `escalate_model` is a model name, or \"\" for none"))?
                .trim()
                .to_string(),
        )
        .filter(|m| !m.is_empty()),
    };
    Ok(AgentCfg {
        provider,
        escalate_model,
        account,
        codex_bin: s("codex_bin").map(PathBuf::from),
        ladder,
        max_retries: doc
            .get("max_retries")
            .and_then(|v| v.as_integer())
            .map_or(1, |n| n.clamp(0, 5) as u32),
        sandbox: match s("sandbox") {
            None => Default::default(),
            Some(v) => {
                mason_adapters::sandbox::SandboxKind::parse(&v).map_err(|e| format!("{id}: {e}"))?
            }
        },
    })
}

/// `[self]` of the project file (Part 13).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelfCfg {
    /// More globs to protect, added to the compiled list (never removing from it).
    pub protect: Vec<String>,
    /// Self-tasks that may be open (not landed, failed or cancelled) at once.
    pub max_open: u32,
    /// Self-tasks that may be filed in a week.
    pub per_week: u32,
    /// Agent steps self-tasks may start per period: the cap on spend. (Token cost is
    /// not recorded yet, so steps are the measure.)
    pub max_steps: u32,
    pub period_h: u64,
    /// After a rollback, no self-task is filed for this long.
    pub cooldown_h: u64,
    /// What a change to a protected file, or one that weakens a check, does at landing.
    pub holds: Holds,
}

/// `[self] holds`: who judges a change to MASON's own rules.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Holds {
    /// Nobody waits. A change that weakens a check goes back to development with what was
    /// found; one that only touches a protected file lands once its tests and review are
    /// green, and the record keeps what it touched. Validity is decided where the context is
    /// (test and review), not by a person at the end.
    #[default]
    Review,
    /// The task waits for the owner's approval of the diff (the Stage 17 behavior).
    Owner,
}

fn parse_self(id: &str, doc: &DocumentMut) -> Result<Option<SelfCfg>, String> {
    let Some(t) = doc.get("self").and_then(|v| v.as_table_like()) else {
        return Ok(None);
    };
    let int = |k: &str, d: u64| -> Result<u64, String> {
        match t.get(k) {
            None => Ok(d),
            Some(v) => v
                .as_integer()
                .filter(|n| *n >= 1)
                .map(|n| n as u64)
                .ok_or_else(|| format!("{id}: `[self] {k}` must be a whole number from 1")),
        }
    };
    let protect: Vec<String> = match t.get("protect") {
        None => vec![],
        Some(v) => v
            .as_array()
            .ok_or_else(|| format!("{id}: `[self] protect` must be a list of globs"))?
            .iter()
            .filter_map(|x| x.as_str().map(String::from))
            .collect(),
    };
    Ok(Some(SelfCfg {
        protect,
        max_open: int("max_open", 3)? as u32,
        per_week: int("per_week", 10)? as u32,
        max_steps: int("max_steps", 40)? as u32,
        period_h: int("period_h", 24)?,
        cooldown_h: int("cooldown_h", 24)?,
        holds: match t.get("holds").and_then(|v| v.as_str()) {
            None | Some("review") => Holds::Review,
            Some("owner") => Holds::Owner,
            Some(other) => {
                return Err(format!(
                    "{id}: `[self] holds` must be \"review\" or \"owner\", not {other:?}"
                ))
            }
        },
    }))
}

/// `[host]`, `[ci]` and `[tracker]` of the project file. Credentials are
/// references (`env:NAME`, `file:/path`), never values.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServicesCfg {
    pub host: Option<HostCfg>,
    pub ci: Option<CiCfg>,
    pub tracker: Option<TrackerCfg>,
    /// `[remote_runs]`: where a run may go instead of this machine (#697).
    pub remote: Option<RemoteCfg>,
}

/// `[remote_runs]` of the project file: runs of suites and checks on GitHub Actions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCfg {
    pub api: String,
    /// `owner/name`.
    pub repo: String,
    /// `env:NAME` or `file:/path`.
    pub token: String,
    /// The workflow file on the base branch that runs on `mason/run/**`.
    pub workflow: String,
    /// What a job runs on when the decision names nothing else.
    pub runs_on: String,
    /// Seconds a run may wait for a runner before it counts as an outage.
    pub pickup_timeout_s: u64,
    /// `favor-local` (#832): a run whose wait for this machine's slots passes this many seconds
    /// may start on GitHub instead.
    pub spill_after_s: u64,
    /// `favor-remote` (#833): GitHub is overloaded when the account has more than this many
    /// jobs queued.
    pub remote_queue_max: u32,
    /// With `fallback = "local"` a run that cannot go remote runs here instead.
    pub fallback_local: bool,
    /// The project's placement mode (#831): `[remote_runs] mode`, else the old `fallback` (`local`
    /// is `favor-remote`, `none` is `remote`). `None`: the project follows the global mode.
    pub mode: Option<String>,
    /// `test_env = { MASON_X = "v" }`: variables every run's `run.json` carries to the runner
    /// (only `MASON_*` names reach it). The owner's configuration.
    pub test_env: Vec<(String, String)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCfg {
    Github {
        api: String,
        repo: String,
        token: String,
    },
    /// An approved project extension that provides `adapter:host:<name>` (`kind = "plugin:<name>"`).
    Plugin { name: String },
    /// A bare repository and a directory of files standing in for a host (tests).
    Fake {
        dir: PathBuf,
        remote: PathBuf,
        checks: Option<String>,
        require_approval: bool,
        merge_queue: bool,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiCfg {
    pub kind: CiKind,
    /// Seconds between looks at a run.
    pub poll_s: u64,
    /// Give up on a run after this long.
    pub timeout_s: u64,
    /// Tell the owner when a pending run's jobs have not changed for this long (seconds;
    /// `0`: never). Only an alert; nothing is cancelled and no verdict changes.
    pub stuck_after_s: u64,
    /// Tell infrastructure reds from real ones (`github-actions` only): only infrastructure
    /// is rerun, a real red goes back to the tasks at once. Off by default.
    pub infra_judge: bool,
    /// Log phrases that also mean infrastructure, added to the built-in list.
    pub infra_patterns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiKind {
    /// An approved project extension that provides `adapter:ci:<name>` (`kind = "plugin:<name>"`).
    Plugin { name: String },
    GithubActions {
        api: String,
        repo: String,
        token: String,
        workflow: Option<String>,
    },
    Command {
        trigger: String,
        status: String,
        /// Cancels a run MASON stops waiting on; optional.
        cancel: Option<String>,
        cwd: PathBuf,
    },
    /// Jenkins: a parameterised job taking `REF` and `SHA`.
    Jenkins {
        url: String,
        job: String,
        token: String,
        user: Option<String>,
        crumb: bool,
    },
    /// Trigger and status, each a command or a URL.
    Generic {
        trigger: mason_adapters::generic::Half,
        status: mason_adapters::generic::Half,
        cwd: PathBuf,
        token: Option<String>,
    },
}

/// What an imported issue becomes. Set only in the machine-side project config
/// (never by an issue's own text). Decision 0046.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportAs {
    /// Waits for the owner's approval before it starts.
    #[default]
    Proposed,
    /// Starts at once, as before.
    Queued,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrackerCfg {
    /// `kind = "plugin:<name>"`: an approved project extension that provides
    /// `adapter:tracker:<name>`; `api`, `repo` and `token` are then empty.
    pub plugin: Option<String>,
    pub api: String,
    pub repo: String,
    pub token: String,
    /// The label the owner puts on an outside issue to have it imported.
    pub label: String,
    pub interval_s: u64,
    /// A reference to the webhook secret (`env:`/`file:`), when a relay delivers.
    pub webhook_secret: Option<String>,
    /// `proposed` (default) or `queued`.
    pub import_as: ImportAs,
    /// When true an issue is imported only if it carries `label`, whoever filed it (a
    /// tracker shared with another pipeline). Default false: the owner's issues also import.
    pub require_label: bool,
}

/// The extension named by `kind = "plugin:<name>"`, if that is what `kind` says.
fn plugin_name(kind: &str) -> Option<&str> {
    kind.strip_prefix("plugin:")
        .filter(|n| crate::config::valid_id(n))
}

fn parse_services(id: &str, doc: &DocumentMut) -> Result<ServicesCfg, String> {
    let table = |k: &str| doc.get(k).and_then(|v| v.as_table_like());
    let get =
        |t: &dyn toml_edit::TableLike, k: &str| t.get(k).and_then(|v| v.as_str()).map(String::from);
    let need = |t: &dyn toml_edit::TableLike, sec: &str, k: &str| {
        get(t, k)
            .filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("{id}: `[{sec}] {k}` is required"))
    };
    let token = |t: &dyn toml_edit::TableLike, sec: &str| -> Result<String, String> {
        let v = need(t, sec, "token")?;
        if v.starts_with("env:") || v.starts_with("file:") {
            Ok(v)
        } else {
            Err(format!("{id}: `[{sec}] token` is a reference, `env:NAME` or `file:/path`, never the token itself"))
        }
    };
    let int = |t: &dyn toml_edit::TableLike, k: &str, d: u64| {
        t.get(k)
            .and_then(|v| v.as_integer())
            .map_or(d, |n| n.max(1) as u64)
    };
    let api = |t: &dyn toml_edit::TableLike| {
        get(t, "api").unwrap_or_else(|| "https://api.github.com".into())
    };
    let mut out = ServicesCfg::default();
    if let Some(t) = table("host") {
        out.host = Some(match need(t, "host", "kind")?.as_str() {
            "github" => HostCfg::Github {
                api: api(t),
                repo: need(t, "host", "repo")?,
                token: token(t, "host")?,
            },
            "fake" => HostCfg::Fake {
                dir: need(t, "host", "dir")?.into(),
                remote: need(t, "host", "remote")?.into(),
                checks: get(t, "checks"),
                require_approval: t
                    .get("require_approval")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
                merge_queue: t
                    .get("merge_queue")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false),
            },
            o if plugin_name(o).is_some() => HostCfg::Plugin {
                name: plugin_name(o).unwrap_or_default().to_string(),
            },
            o => {
                return Err(format!(
                    "{id}: `[host] kind` is `github`, `fake` or `plugin:<name>`, not {o:?}"
                ))
            }
        });
    }
    if let Some(t) = table("ci") {
        let kind = match need(t, "ci", "kind")?.as_str() {
            "github-actions" => CiKind::GithubActions {
                api: api(t),
                repo: need(t, "ci", "repo")?,
                token: token(t, "ci")?,
                workflow: get(t, "workflow"),
            },
            "command" => CiKind::Command {
                trigger: need(t, "ci", "trigger")?,
                status: need(t, "ci", "status")?,
                cancel: get(t, "cancel"),
                cwd: get(t, "cwd").map_or_else(|| PathBuf::from("."), PathBuf::from),
            },
            "jenkins" => CiKind::Jenkins {
                url: need(t, "ci", "url")?,
                job: need(t, "ci", "job")?,
                token: token(t, "ci")?,
                user: get(t, "user"),
                crumb: t.get("crumb").and_then(|v| v.as_bool()).unwrap_or(false),
            },
            "generic" => {
                use mason_adapters::generic::Half;
                let half = |c: &str, u: &str| -> Result<Half, String> {
                    match (get(t, c), get(t, u)) {
                        (Some(c), None) if !c.trim().is_empty() => Ok(Half::Command(c)),
                        (None, Some(u)) if !u.trim().is_empty() => Ok(Half::Url(u)),
                        _ => Err(format!(
                            "{id}: `[ci]` needs exactly one of `{c}` (a command) or `{u}` (a URL)"
                        )),
                    }
                };
                let (trigger, status) = (half("trigger", "trigger_url")?, half("status", "status_url")?);
                let tok = get(t, "token");
                if tok
                    .as_ref()
                    .is_some_and(|v| !(v.starts_with("env:") || v.starts_with("file:")))
                {
                    return Err(format!(
                        "{id}: `[ci] token` is a reference, `env:NAME` or `file:/path`, never the token itself"
                    ));
                }
                CiKind::Generic {
                    trigger,
                    status,
                    cwd: get(t, "cwd").map_or_else(|| PathBuf::from("."), PathBuf::from),
                    token: tok,
                }
            }
            o if plugin_name(o).is_some() => CiKind::Plugin {
                name: plugin_name(o).unwrap_or_default().to_string(),
            },
            o => {
                return Err(format!(
                    "{id}: `[ci] kind` is `github-actions`, `jenkins`, `generic`, `command` or `plugin:<name>`, not {o:?}"
                ))
            }
        };
        let infra_judge = t
            .get("infra_judge")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if infra_judge && !matches!(kind, CiKind::GithubActions { .. }) {
            return Err(format!(
                "{id}: `[ci] infra_judge` works only with `kind = \"github-actions\"`"
            ));
        }
        let infra_patterns: Vec<String> = t
            .get("infra_patterns")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        if !infra_patterns.is_empty() && !infra_judge {
            return Err(format!(
                "{id}: `[ci] infra_patterns` needs `infra_judge = true`"
            ));
        }
        out.ci = Some(CiCfg {
            infra_judge,
            infra_patterns,
            kind,
            poll_s: int(t, "poll_s", 20),
            timeout_s: int(t, "timeout_s", 7200),
            stuck_after_s: match t.get("stuck_after") {
                None => 1800,
                Some(v) => match (v.as_integer(), v.as_str()) {
                    (Some(n), _) if n >= 0 => n as u64,
                    (_, Some(s)) => match s.trim() {
                        "0" => 0,
                        s => mason_landing::config::parse_duration(s)
                            .map(|d| d.as_secs())
                            .ok_or_else(|| {
                                format!("{id}: `[ci] stuck_after` is a duration like \"30m\" or 0")
                            })?,
                    },
                    _ => {
                        return Err(format!(
                            "{id}: `[ci] stuck_after` is a duration like \"30m\" or 0"
                        ))
                    }
                },
            },
        });
    }
    if table("remote").is_some() {
        return Err(format!(
            "{id}: `[remote]` would clash with the git remote key `remote`; put remote-run settings under `[remote_runs]`"
        ));
    }
    if let Some(t) = table("remote_runs") {
        match need(t, "remote_runs", "kind")?.as_str() {
            "github-actions" => {}
            o => {
                return Err(format!(
                    "{id}: `[remote_runs] kind` is `github-actions`, not {o:?}"
                ))
            }
        }
        // The repository, token and API default to `[ci]`'s when that is GitHub Actions.
        let ci_gh = match out.ci.as_ref().map(|c| &c.kind) {
            Some(CiKind::GithubActions {
                api, repo, token, ..
            }) => Some((api.clone(), repo.clone(), token.clone())),
            _ => None,
        };
        let repo = match get(t, "repo") {
            Some(r) if !r.trim().is_empty() => r,
            _ => ci_gh
                .as_ref()
                .map(|c| c.1.clone())
                .ok_or_else(|| format!("{id}: `[remote_runs] repo` is required"))?,
        };
        let token = match get(t, "token") {
            Some(v) if v.starts_with("env:") || v.starts_with("file:") => v,
            Some(_) => {
                return Err(format!(
                    "{id}: `[remote_runs] token` is a reference, `env:NAME` or `file:/path`, never the token itself"
                ))
            }
            None => ci_gh
                .as_ref()
                .map(|c| c.2.clone())
                .ok_or_else(|| format!("{id}: `[remote_runs] token` is required"))?,
        };
        let fallback = match get(t, "fallback").as_deref() {
            None => None,
            Some(o @ ("none" | "local")) => Some(o.to_string()),
            Some(o) => {
                return Err(format!(
                    "{id}: `[remote_runs] fallback` is `local` or `none`, not {o:?}"
                ))
            }
        };
        let mode = match get(t, "mode").as_deref() {
            Some(m @ ("local" | "remote" | "favor-local" | "favor-remote")) => Some(m.to_string()),
            Some(o) => {
                return Err(format!(
                    "{id}: `[remote_runs] mode` is `local`, `remote`, `favor-local` or `favor-remote`, not {o:?}"
                ))
            }
            None => match fallback.as_deref() {
                Some("local") => Some("favor-remote".to_string()),
                Some(_) => Some("remote".to_string()),
                None => None,
            },
        };
        let fallback_local =
            fallback.as_deref() == Some("local") || mode.as_deref() == Some("favor-remote");
        out.remote = Some(RemoteCfg {
            api: get(t, "api")
                .or_else(|| ci_gh.map(|c| c.0))
                .unwrap_or_else(|| "https://api.github.com".into()),
            repo,
            token,
            workflow: get(t, "workflow").unwrap_or_else(|| "mason-run.yml".into()),
            runs_on: get(t, "runs_on").unwrap_or_else(|| "ubuntu-latest".into()),
            pickup_timeout_s: int(t, "pickup_timeout", 1200),
            spill_after_s: int(t, "spill_after", 120),
            remote_queue_max: int(t, "remote_queue_max", 4) as u32,
            fallback_local,
            mode,
            test_env: match t.get("test_env") {
                None => vec![],
                Some(v) => {
                    let bad = || {
                        format!("{id}: `[remote_runs] test_env` is a table of `MASON_*` names to strings")
                    };
                    let tb = v.as_table_like().ok_or_else(bad)?;
                    let mut out = Vec::new();
                    for (k, v) in tb.iter() {
                        let s = v.as_str().ok_or_else(bad)?;
                        if !k.starts_with("MASON_")
                            || !k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                        {
                            return Err(bad());
                        }
                        out.push((k.to_string(), s.to_string()));
                    }
                    out
                }
            },
        });
    }
    if let Some(t) = table("tracker") {
        let tracker_plugin = match need(t, "tracker", "kind")?.as_str() {
            "github" => None,
            o if plugin_name(o).is_some() => plugin_name(o).map(String::from),
            o => return Err(format!("{id}: `[tracker] kind` is `github` or `plugin:<name>` (the local tracker needs no section), not {o:?}")),
        };
        let secret = get(t, "webhook_secret");
        if secret
            .as_ref()
            .is_some_and(|s| !(s.starts_with("env:") || s.starts_with("file:")))
        {
            return Err(format!(
                "{id}: `[tracker] webhook_secret` is a reference, `env:NAME` or `file:/path`"
            ));
        }
        let import_as = match get(t, "import_as").as_deref() {
            None | Some("proposed") => ImportAs::Proposed,
            Some("queued") => ImportAs::Queued,
            Some(o) => {
                return Err(format!(
                    "{id}: `[tracker] import_as` is `proposed` or `queued`, not {o:?}"
                ))
            }
        };
        if t.get("import_as").is_some_and(|v| v.as_str().is_none()) {
            return Err(format!(
                "{id}: `[tracker] import_as` is `proposed` or `queued`"
            ));
        }
        let require_label = match t.get("require_label") {
            None => false,
            Some(v) => v
                .as_bool()
                .ok_or_else(|| format!("{id}: `[tracker] require_label` is `true` or `false`"))?,
        };
        let (api, repo, token) = if tracker_plugin.is_some() {
            (String::new(), String::new(), String::new())
        } else {
            (api(t), need(t, "tracker", "repo")?, token(t, "tracker")?)
        };
        out.tracker = Some(TrackerCfg {
            plugin: tracker_plugin,
            api,
            repo,
            token,
            label: get(t, "label").unwrap_or_else(|| "mason".into()),
            interval_s: int(t, "interval_s", 60),
            webhook_secret: secret,
            import_as,
            require_label,
        });
    }
    Ok(out)
}

impl ProjectCfg {
    /// Provider, retry ladder and sandbox, from the overlay.
    pub fn agent(&self) -> AgentCfg {
        self.overlay_text
            .parse::<DocumentMut>()
            .ok()
            .and_then(|d| parse_agent(&self.id, &d).ok())
            .unwrap_or_default()
    }
    pub fn branch(&self, task: u64) -> String {
        format!("mason/{}/t{task}", self.id)
    }
    pub fn worktree(&self, cfg: &DaemonConfig, task: u64) -> PathBuf {
        cfg.worktrees_dir().join(&self.id).join(task.to_string())
    }
    pub fn landing_worktree(&self, cfg: &DaemonConfig) -> PathBuf {
        cfg.landing_dir().join(&self.id)
    }
    /// The ref work starts from and lands onto.
    /// The project file's own run settings, as `mason config show` lists them under the
    /// steps: timeouts, the agent idle rules, the stall watcher, retries. These come from
    /// `projects/<id>.toml`, not the definition, and an agent checking for them in the
    /// definition's settings alone finds nothing (#170 asked the owner twice on 2026-11-14).
    /// A zero that turns a rule off is shown as `0 (off)`.
    pub fn file_settings(&self) -> Vec<(&'static str, String)> {
        let secs = |d: Duration| d.as_secs().to_string();
        let off_at_zero = |d: Duration| match d.as_secs() {
            0 => "0 (off)".to_string(),
            n => n.to_string(),
        };
        vec![
            ("step_timeout_s", secs(self.step_timeout)),
            ("test_timeout_s", secs(self.test_timeout)),
            ("tool_timeout_s", secs(self.tool_timeout)),
            ("agent_idle_alert_s", off_at_zero(self.agent_idle_alert)),
            ("agent_idle_stop_s", off_at_zero(self.agent_idle_stop)),
            ("cont_after_s", off_at_zero(self.cont_after)),
            ("kill_after_s", off_at_zero(self.kill_after)),
            ("test_reruns", self.test_reruns.to_string()),
            ("auto_retry", self.auto_retry.to_string()),
            ("diagnose", self.diagnose.to_string()),
            (
                "diagnose_model",
                self.diagnose_model.clone().unwrap_or_default(),
            ),
            (
                "diagnose_effort",
                self.diagnose_effort.clone().unwrap_or_default(),
            ),
            ("priority_jump", self.priority_jump.to_string()),
            ("reuse_green", self.reuse_green.to_string()),
        ]
    }

    pub fn base_ref(&self) -> String {
        match &self.remote {
            Some(r) => format!("{r}/{}", self.base),
            None => self.base.clone(),
        }
    }
}

pub(crate) fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn parse_project(id: &str, text: &str) -> Result<ProjectCfg, String> {
    if !valid_id(id) {
        return Err(format!(
            "project id {id:?} must be lowercase letters, digits, - and _"
        ));
    }
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| format!("{id}: not valid TOML: {e}"))?;
    let s = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(String::from);
    let need = |k: &str| {
        s(k).filter(|v| !v.trim().is_empty())
            .ok_or_else(|| format!("{id}: `{k}` is required"))
    };
    let secs = |k: &str, d: u64| {
        doc.get(k)
            .and_then(|v| v.as_integer())
            .map_or(d, |n| n.max(1) as u64)
    };
    let mut agent_env = vec![];
    if let Some(t) = doc.get("agent_env").and_then(|v| v.as_table_like()) {
        for (k, v) in t.iter() {
            let v = v
                .as_str()
                .ok_or_else(|| format!("{id}: agent_env.{k} must be a string"))?;
            agent_env.push((k.to_string(), v.to_string()));
        }
    }
    let base = need("base")?;
    if !mason_landing::git::valid_branch(&base) {
        return Err(format!(
            "{id}: base branch {base:?} is not a plain branch name"
        ));
    }
    let agent = parse_agent(id, &doc)?;
    let diagnose_model = match doc.get("diagnose_model") {
        None => None,
        Some(v) => match v.as_str().map(str::trim).filter(|m| !m.is_empty()) {
            Some(m) => Some(m.to_string()),
            None => return Err(format!("{id}: `diagnose_model` must be a non-empty string")),
        },
    };
    let diagnose_effort = match doc.get("diagnose_effort") {
        None => None,
        Some(v) => {
            let efforts = crate::tiermap::effort_levels(agent.provider);
            match v.as_str() {
                Some(e) if efforts.iter().any(|x| x == e) => Some(e.to_string()),
                Some(e) => {
                    return Err(mason_definition::tiers::bad_effort(
                        &format!("{id}: `diagnose_effort`"),
                        e,
                        &efforts,
                    ))
                }
                None => return Err(format!("{id}: `diagnose_effort` must be a string")),
            }
        }
    };
    crate::proposals::parse_cfg(id, &doc)?;
    crate::introspect::parse_cfg(id, &doc)?;
    crate::schedule::parse(id, &doc)?;
    Ok(ProjectCfg {
        account: agent
            .account
            .unwrap_or_else(|| crate::providers::adapter_name(agent.provider).to_string()),
        id: id.to_string(),
        path: PathBuf::from(need("path")?),
        base,
        remote: s("remote").filter(|r| !r.is_empty()),
        test_cmd: need("test")?,
        claude_bin: s("claude_bin").map(PathBuf::from),
        model: s("model"),
        agent_env,
        tool_timeout: Duration::from_secs(secs("tool_timeout_s", 600)),
        step_timeout: Duration::from_secs(secs("step_timeout_s", 4 * 3600)),
        agent_idle_alert: Duration::from_secs(
            doc.get("agent_idle_alert_s")
                .and_then(|v| v.as_integer())
                .map_or(2700, |n| n.max(0) as u64),
        ),
        agent_idle_stop: Duration::from_secs(
            doc.get("agent_idle_stop_s")
                .and_then(|v| v.as_integer())
                .map_or(5400, |n| n.max(0) as u64),
        ),
        cont_after: Duration::from_secs(secs("cont_after_s", 0)),
        kill_after: Duration::from_secs(secs("kill_after_s", 0)),
        test_timeout: Duration::from_secs(secs("test_timeout_s", 1800)),
        diagnose: doc
            .get("diagnose")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        diagnose_model,
        diagnose_effort,
        auto_retry: doc
            .get("auto_retry")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        max_test_rounds: doc
            .get("max_test_rounds")
            .and_then(|v| v.as_integer())
            .map(|n| n.max(1) as u32),
        test_reruns: doc
            .get("test_reruns")
            .and_then(|v| v.as_integer())
            .map_or(0, |n| n.clamp(0, 3) as u32),
        rerun_filter: match s("rerun_filter").filter(|f| !f.trim().is_empty()) {
            Some(f) => {
                crate::rerun::check_filter(&f).map_err(|e| format!("{id}: {e}"))?;
                Some(f)
            }
            None => None,
        },
        task_cache_keep_days: doc
            .get("task_cache_keep_days")
            .and_then(|v| v.as_integer())
            .map(|n| n.max(0) as u64),
        task_cache_seed: doc
            .get("task_cache_seed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        task_cache: match s("task_cache").filter(|t| !t.trim().is_empty()) {
            Some(t) => {
                crate::taskcache::check_template(&t).map_err(|e| format!("{id}: {e}"))?;
                Some(t)
            }
            None => None,
        },
        clarity_classifier: s("clarity_classifier").filter(|c| !c.trim().is_empty()),
        card_rewriter: s("card_rewriter").filter(|c| !c.trim().is_empty()),
        reuse_green: doc
            .get("reuse_green")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        reuse_full_hours: doc
            .get("reuse_full_hours")
            .and_then(|v| v.as_integer())
            .map_or(24, |n| n.max(0) as u64),
        small_run_s: doc
            .get("small_run_s")
            .and_then(|v| v.as_integer())
            .map_or(180, |n| n.max(0) as u64),
        hold_back_secs: doc
            .get("hold_back_secs")
            .and_then(|v| v.as_integer())
            .map_or(7200, |n| n.max(0) as u64),
        park_extra: doc
            .get("park_extra")
            .and_then(|v| v.as_integer())
            .map(|n| n.clamp(0, u32::MAX as i64) as u32),
        hold_back_ignore: doc
            .get("hold_back_ignore")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default(),
        priority_jump: doc
            .get("priority_jumps_progress")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        config_dir: PathBuf::new(),
        overlay_text: text.to_string(),
        services: parse_services(id, &doc)?,
        self_cfg: parse_self(id, &doc)?,
    })
}

/// Top-level keys of a registration file that are fixed when masond starts: a
/// reload that changes one is refused.
pub const FIXED_KEYS: &[&str] = &["path", "base", "remote", "self"];

/// The top-level keys (tables included) whose text differs between two
/// registration files, sorted. Text that does not parse counts as empty.
pub fn changed_keys(old: &str, new: &str) -> Vec<String> {
    let map = |t: &str| -> std::collections::BTreeMap<String, String> {
        t.parse::<DocumentMut>()
            .map(|d| {
                d.iter()
                    .map(|(k, v)| (k.to_string(), v.to_string().trim().to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (a, b) = (map(old), map(new));
    let mut keys: Vec<String> = a.keys().chain(b.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.retain(|k| a.get(k) != b.get(k));
    keys
}

/// One project's registration file, parsed and its account resolved.
pub fn load_project(config_dir: &Path, id: &str) -> Result<ProjectCfg, String> {
    let file = config_dir.join("projects").join(format!("{id}.toml"));
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut c = parse_project(id, &text)?;
    c.config_dir = config_dir.to_path_buf();
    let accounts = crate::providers::load(config_dir)?;
    let a = c.agent();
    c.account = crate::providers::resolve_account(&accounts, a.provider, a.account.as_deref())
        .map_err(|e| format!("{}: {e}", c.id))?;
    Ok(c)
}

/// Every `*.toml` under `<config_dir>/projects`. Errors name the file.
pub fn load_projects(config_dir: &Path) -> Result<Vec<ProjectCfg>, Vec<String>> {
    let dir = config_dir.join("projects");
    let mut out = vec![];
    let mut errs = vec![];
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    let accounts = match crate::providers::load(config_dir) {
        Ok(a) => a,
        Err(e) => return Err(vec![e]),
    };
    for p in files {
        let id = p
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        match std::fs::read_to_string(&p) {
            Ok(t) => match parse_project(&id, &t) {
                Ok(mut c) => {
                    c.config_dir = config_dir.to_path_buf();
                    let a = c.agent();
                    match crate::providers::resolve_account(
                        &accounts,
                        a.provider,
                        a.account.as_deref(),
                    ) {
                        Ok(n) => {
                            c.account = n;
                            out.push(c);
                        }
                        Err(e) => errs.push(format!("{}: {e}", c.id)),
                    }
                }
                Err(e) => errs.push(e),
            },
            Err(e) => errs.push(format!("{}: {e}", p.display())),
        }
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimal_project_parses_with_defaults() {
        let p = parse_project(
            "web-api",
            "path = \"/r\"\nbase = \"develop\"\ntest = \"make test\"\n",
        )
        .unwrap();
        assert_eq!(
            (p.base.as_str(), p.test_cmd.as_str(), p.remote.clone()),
            ("develop", "make test", None)
        );
        assert_eq!(p.max_test_rounds, None);
        assert_eq!(p.branch(7), "mason/web-api/t7");
        assert_eq!(p.base_ref(), "develop");
    }

    #[test]
    fn the_project_files_own_settings_are_listed_with_an_off_zero_named() {
        // `mason config show` printed only the definition's settings, so an agent looking for
        // the stall watcher there found nothing and asked the owner twice (#170, 2026-11-14).
        let p = parse_project(
            "star",
            "path = \"/r\"\nbase = \"develop\"\ntest = \"t\"\ncont_after_s = 600\nkill_after_s = 900\n",
        )
        .unwrap();
        let s = p.file_settings();
        let get = |k: &str| s.iter().find(|(key, _)| *key == k).map(|(_, v)| v.as_str());
        assert_eq!(get("cont_after_s"), Some("600"));
        assert_eq!(get("kill_after_s"), Some("900"));
        assert_eq!(get("step_timeout_s"), Some("14400"));
        let plain =
            parse_project("p", "path = \"/r\"\nbase = \"develop\"\ntest = \"t\"\n").unwrap();
        let s = plain.file_settings();
        assert!(
            s.contains(&("kill_after_s", "0 (off)".to_string())),
            "{s:?}"
        );
    }

    #[test]
    fn import_as_defaults_to_proposed_and_is_validated() {
        let base = "path=\"/r\"\nbase=\"main\"\ntest=\"t\"\n[tracker]\nkind=\"github\"\nrepo=\"o/r\"\ntoken=\"env:X\"\n";
        let t = |extra: &str| {
            parse_project("p", &format!("{base}{extra}"))
                .map(|p| p.services.tracker.unwrap().import_as)
        };
        assert_eq!(t("").unwrap(), ImportAs::Proposed);
        assert_eq!(t("import_as=\"queued\"\n").unwrap(), ImportAs::Queued);
        assert_eq!(t("import_as=\"proposed\"\n").unwrap(), ImportAs::Proposed);
        let e = t("import_as=\"now\"\n").unwrap_err();
        assert!(e.contains("import_as"), "{e}");
        assert!(t("import_as=3\n").unwrap_err().contains("import_as"));
    }

    #[test]
    fn require_label_defaults_to_false_and_is_validated() {
        let base = "path=\"/r\"\nbase=\"main\"\ntest=\"t\"\n[tracker]\nkind=\"github\"\nrepo=\"o/r\"\ntoken=\"env:X\"\n";
        let t = |extra: &str| {
            parse_project("p", &format!("{base}{extra}"))
                .map(|p| p.services.tracker.unwrap().require_label)
        };
        assert!(!t("").unwrap());
        assert!(t("require_label=true\n").unwrap());
        assert!(!t("require_label=false\n").unwrap());
        for bad in ["require_label=\"yes\"\n", "require_label=3\n"] {
            let e = t(bad).unwrap_err();
            assert!(e.contains("[tracker] require_label"), "{e}");
        }
    }

    #[test]
    fn remote_env_and_limits_are_read() {
        let t = "path=\"/r\"\nbase=\"main\"\ntest=\"t\"\nremote=\"origin\"\nmodel=\"haiku\"\nstep_timeout_s=60\n[agent_env]\nFOO=\"bar\"\n";
        let p = parse_project("p", t).unwrap();
        assert_eq!(p.base_ref(), "origin/main");
        assert_eq!(p.agent_env, vec![("FOO".to_string(), "bar".to_string())]);
        assert_eq!(p.step_timeout, Duration::from_secs(60));
        assert_eq!(p.model.as_deref(), Some("haiku"));
    }

    #[test]
    fn stall_keys_are_off_unless_set() {
        let base = "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n";
        let p = parse_project("p", base).unwrap();
        assert_eq!(
            (p.cont_after, p.kill_after),
            (Duration::ZERO, Duration::ZERO)
        );
        let p =
            parse_project("p", &format!("{base}cont_after_s=600\nkill_after_s=1800\n")).unwrap();
        assert_eq!(
            (p.cont_after, p.kill_after),
            (Duration::from_secs(600), Duration::from_secs(1800))
        );
    }

    #[test]
    fn agent_keys_default_to_claude_with_no_ladder_and_no_sandbox() {
        let p = parse_project("p", "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n").unwrap();
        let a = p.agent();
        assert_eq!(a.provider, Provider::Claude);
        assert!(a.ladder.is_empty() && a.codex_bin.is_none());
        assert_eq!(a.max_retries, 1);
        assert_eq!(a.sandbox, mason_adapters::sandbox::SandboxKind::None);
    }

    #[test]
    fn escalate_model_defaults_to_opus_for_claude_and_can_be_changed_or_turned_off() {
        let base = "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n";
        let m = |extra: &str| {
            parse_project("p", &format!("{base}{extra}\n"))
                .unwrap()
                .agent()
                .escalate_model
        };
        assert_eq!(m(""), Some("opus".to_string()));
        assert_eq!(m("escalate_model = \"fable\""), Some("fable".to_string()));
        assert_eq!(m("escalate_model = \"\""), None);
        assert_eq!(m("provider = \"codex\""), None);
    }

    #[test]
    fn agent_keys_are_read_and_mistakes_named() {
        let base = "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n";
        let p = parse_project(
            "p",
            &format!("{base}provider=\"codex\"\nladder=[\"a\",\"b\"]\nmax_retries=2\nsandbox=\"bubblewrap\"\ncodex_bin=\"/x/codex\"\n"),
        )
        .unwrap();
        let a = p.agent();
        assert_eq!(
            (a.provider, a.ladder.len(), a.max_retries),
            (Provider::Codex, 2, 2)
        );
        assert_eq!(a.sandbox, mason_adapters::sandbox::SandboxKind::Bubblewrap);
        for (extra, want) in [
            ("provider=\"gemini\"", "`claude` or `codex`"),
            ("ladder=\"a\"", "list of model names"),
            (
                "sandbox=\"docker\"",
                "`none`, `sandbox-exec` or `bubblewrap`",
            ),
        ] {
            let e = parse_project("p", &format!("{base}{extra}\n")).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn host_ci_and_tracker_can_be_a_plugin_and_other_kinds_name_the_choices() {
        let base = "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n";
        let p = parse_project(
            "p",
            &format!(
                "{base}[host]\nkind=\"plugin:acme\"\n[ci]\nkind=\"plugin:acme-ci\"\n[tracker]\nkind=\"plugin:acme\"\n"
            ),
        )
        .unwrap();
        assert_eq!(
            p.services.host,
            Some(HostCfg::Plugin {
                name: "acme".into()
            })
        );
        assert_eq!(
            p.services.ci.unwrap().kind,
            CiKind::Plugin {
                name: "acme-ci".into()
            }
        );
        // A plugin tracker needs no repo or token.
        let t = p.services.tracker.unwrap();
        assert_eq!(t.plugin.as_deref(), Some("acme"));
        assert!(t.repo.is_empty() && t.token.is_empty());
        for (section, want) in [
            ("host", "`github`, `fake` or `plugin:<name>`"),
            ("ci", "`command` or `plugin:<name>`"),
            ("tracker", "`github` or `plugin:<name>`"),
        ] {
            for kind in ["gitlab", "plugin:", "plugin:Bad Name"] {
                let e = parse_project("p", &format!("{base}[{section}]\nkind=\"{kind}\"\n"))
                    .unwrap_err();
                assert!(e.contains(want), "{section} {kind}: {e}");
            }
        }
    }

    #[test]
    fn remote_runs_sit_beside_the_git_remote_key() {
        let f = "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\nremote=\"origin\"\n[ci]\nkind=\"github-actions\"\nrepo=\"o/r\"\ntoken=\"env:T\"\n[remote_runs]\nkind=\"github-actions\"\n";
        let p = parse_project("p", f).unwrap();
        assert_eq!(p.remote.as_deref(), Some("origin"));
        assert_eq!(p.services.remote.unwrap().repo, "o/r");
    }

    #[test]
    fn the_remote_section_defaults_to_the_ci_repository_and_token() {
        let base = "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n";
        let ci = "[ci]\nkind=\"github-actions\"\nrepo=\"o/r\"\ntoken=\"env:T\"\n";
        let rem = "[remote_runs]\nkind=\"github-actions\"\n";
        let p = parse_project("p", &format!("{base}{ci}{rem}")).unwrap();
        let r = p.services.remote.unwrap();
        assert_eq!(
            (
                r.repo.as_str(),
                r.token.as_str(),
                r.workflow.as_str(),
                r.runs_on.as_str(),
                r.pickup_timeout_s,
                r.fallback_local
            ),
            (
                "o/r",
                "env:T",
                "mason-run.yml",
                "ubuntu-latest",
                1200,
                false
            )
        );
        let own = "[remote_runs]\nkind=\"github-actions\"\nrepo=\"a/b\"\ntoken=\"file:/t\"\nworkflow=\"w.yml\"\nruns_on=\"macos-latest\"\npickup_timeout=60\nfallback=\"local\"\ntest_env={ MASON_TEST_MIN_WAIT_S = \"180\" }\n";
        let r = parse_project("p", &format!("{base}{ci}{own}"))
            .unwrap()
            .services
            .remote
            .unwrap();
        assert_eq!(
            (
                r.repo.as_str(),
                r.token.as_str(),
                r.workflow.as_str(),
                r.runs_on.as_str()
            ),
            ("a/b", "file:/t", "w.yml", "macos-latest")
        );
        assert!(r.fallback_local && r.pickup_timeout_s == 60 && r.spill_after_s == 120);
        assert_eq!(
            r.test_env,
            vec![("MASON_TEST_MIN_WAIT_S".to_string(), "180".to_string())]
        );
        // Without `[ci]` there is nothing to default from; a literal token and unknown values
        // are refused.
        for (bad, want) in [
            (
                format!("{base}[remote]\nkind=\"github-actions\"\n"),
                "would clash with the git remote key",
            ),
            (format!("{base}{rem}"), "`[remote_runs] repo` is required"),
            (
                format!(
                    "{base}[remote_runs]\nkind=\"github-actions\"\nrepo=\"a/b\"\ntoken=\"abc\"\n"
                ),
                "a reference",
            ),
            (
                format!("{base}{ci}[remote_runs]\nkind=\"github-actions\"\nfallback=\"x\"\n"),
                "fallback",
            ),
            (
                format!("{base}{ci}[remote_runs]\nkind=\"jenkins\"\n"),
                "`[remote_runs] kind`",
            ),
        ] {
            let e = parse_project("p", &bad).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn ci_kinds_jenkins_and_generic_parse_and_refuse_a_literal_token() {
        let base = "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n";
        let j = parse_project("p", &format!("{base}[ci]\nkind=\"jenkins\"\nurl=\"https://ci\"\njob=\"a/b\"\ntoken=\"env:J\"\ncrumb=true\n")).unwrap();
        assert!(matches!(
            j.services.ci.unwrap().kind,
            CiKind::Jenkins { crumb: true, .. }
        ));
        let g = parse_project("p", &format!("{base}[ci]\nkind=\"generic\"\ntrigger=\"echo\"\nstatus_url=\"https://x/{{run}}\"\n")).unwrap();
        assert!(matches!(
            g.services.ci.unwrap().kind,
            CiKind::Generic { .. }
        ));
        for (ci, want) in [
            (
                "kind=\"jenkins\"\nurl=\"u\"\njob=\"j\"\ntoken=\"secret\"",
                "never the token itself",
            ),
            (
                "kind=\"generic\"\ntrigger=\"a\"\ntrigger_url=\"b\"\nstatus=\"c\"",
                "exactly one of",
            ),
            (
                "kind=\"generic\"\ntrigger=\"a\"\nstatus=\"c\"\ntoken=\"raw\"",
                "never the token itself",
            ),
        ] {
            let e = parse_project("p", &format!("{base}[ci]\n{ci}\n")).unwrap_err();
            assert!(e.contains(want), "{e}");
        }
    }

    #[test]
    fn the_infrastructure_judge_is_off_by_default_and_github_only() {
        let base = "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n";
        let gh = "[ci]\nkind=\"github-actions\"\nrepo=\"o/r\"\ntoken=\"env:T\"\n";
        let c = parse_project("p", &format!("{base}{gh}"))
            .unwrap()
            .services
            .ci
            .unwrap();
        assert!(!c.infra_judge && c.infra_patterns.is_empty());
        let c = parse_project(
            "p",
            &format!("{base}{gh}infra_judge=true\ninfra_patterns=[\"sim flake\"]\n"),
        )
        .unwrap()
        .services
        .ci
        .unwrap();
        assert!(c.infra_judge);
        assert_eq!(c.infra_patterns, ["sim flake"]);
        let e = parse_project("p", &format!("{base}{gh}infra_patterns=[\"x\"]\n")).unwrap_err();
        assert!(e.contains("needs `infra_judge = true`"), "{e}");
        let e = parse_project(
            "p",
            &format!(
                "{base}[ci]\nkind=\"command\"\ntrigger=\"a\"\nstatus=\"b\"\ninfra_judge=true\n"
            ),
        )
        .unwrap_err();
        assert!(e.contains("github-actions"), "{e}");
    }

    #[test]
    fn mistakes_are_named() {
        for (id, t, want) in [
            ("p", "base=\"m\"\ntest=\"t\"\n", "`path` is required"),
            ("p", "path=\"/r\"\ntest=\"t\"\n", "`base` is required"),
            ("p", "path=\"/r\"\nbase=\"m\"\n", "`test` is required"),
            (
                "p",
                "path=\"/r\"\nbase=\"-rf\"\ntest=\"t\"\n",
                "not a plain branch name",
            ),
            (
                "P x",
                "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n",
                "must be lowercase",
            ),
            ("p", "path = = =", "not valid TOML"),
        ] {
            let e = parse_project(id, t).unwrap_err();
            assert!(e.contains(want), "{e:?} should contain {want:?}");
        }
    }

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir.join("projects")).unwrap();
        let f = if name == "providers" {
            dir.join("providers.toml")
        } else {
            dir.join("projects").join(format!("{name}.toml"))
        };
        std::fs::write(f, text).unwrap();
    }

    const PROJ: &str = "path=\"/r\"\nbase=\"main\"\ntest=\"t\"\n";

    #[test]
    fn accounts_resolve_at_load_and_a_missing_one_is_refused_by_name() {
        let d = tempfile::tempdir().unwrap();
        // No providers.toml: the one default account.
        write(d.path(), "p", PROJ);
        assert_eq!(load_projects(d.path()).unwrap()[0].account, "claude");
        // A project on codex with no codex account: refused, naming the fix.
        write(d.path(), "q", &format!("{PROJ}provider=\"codex\"\n"));
        let e = load_projects(d.path()).unwrap_err().join("\n");
        assert!(e.contains("q:") && e.contains("adapter = \"codex\""), "{e}");
        write(
            d.path(),
            "providers",
            "[provider.a]\nadapter=\"claude\"\nbilling=\"subscription\"\n[provider.x]\nadapter=\"codex\"\nbilling=\"api\"\n",
        );
        let ps = load_projects(d.path()).unwrap();
        let of = |id: &str| ps.iter().find(|p| p.id == id).unwrap().account.clone();
        assert_eq!((of("p"), of("q")), ("a".into(), "x".into()));
        // Naming a missing account.
        write(d.path(), "r", &format!("{PROJ}account=\"missing\"\n"));
        let e = load_projects(d.path()).unwrap_err().join("\n");
        assert!(e.contains("r:") && e.contains("\"missing\""), "{e}");
        // A bad providers.toml fails the load.
        write(
            d.path(),
            "providers",
            "[provider.a]\nadapter=\"claude\"\nbilling=\"x\"\n",
        );
        assert!(load_projects(d.path()).is_err());
    }

    #[test]
    fn a_node_is_off_by_default_and_bad_values_name_the_setting() {
        use super::{parse_node, NodeCfg};
        assert_eq!(parse_node("").unwrap(), NodeCfg::default());
        assert!(!NodeCfg::default().serve);
        let n = parse_node("[node]\nserve = true\nport = 9000\nbind = [\"10.0.0.2\"]\n").unwrap();
        assert!(n.serve && n.port == 9000 && n.bind == [std::net::IpAddr::from([10, 0, 0, 2])]);
        for (bad, names) in [
            ("[node]\nserve = 1\n", "node.serve"),
            ("[node]\nport = 70000\n", "node.port"),
            ("[node]\nbind = []\n", "node.bind"),
            ("[node]\nbind = [\"x\"]\n", "node.bind"),
            ("[node]\ntoken = \"x\"\n", "node.token"),
            ("node = 3\n", "`node`"),
        ] {
            let e = parse_node(bad).unwrap_err();
            assert!(e.contains(names) && e.contains("daemon.toml"), "{bad}: {e}");
        }
        // The port reader leaves both tables alone.
        assert_eq!(
            super::parse_daemon_port("[node]\nserve = true\n[nodes]\nenabled = true\n").unwrap(),
            None
        );
    }

    #[test]
    fn the_coordinators_nodes_tables_layer_default_then_the_node() {
        use super::{parse_nodes, NodeSettings, OnSwitchOff, Prefer};
        let c = parse_nodes("").unwrap();
        assert!(!c.enabled && c.prefer == Prefer::Nodes && c.projects.is_none());
        assert_eq!(c.settings_for("studio"), NodeSettings::default());
        let c = parse_nodes(
            "[nodes]\nenabled = true\nprefer = \"github\"\nstatic = [\"studio.local:7425\", \"127.0.0.1:1\"]\n\
             status_every_s = 2\nlost_after_polls = 3\nprojects = [\"mason\"]\nexclude = [\"star\"]\n\
             [nodes.default]\nmax_jobs = 4\non_switch_off = \"cancel\"\n[nodes.studio]\nmax_jobs = 6\nmax_die_c = 90\n",
        )
        .unwrap();
        assert!(
            c.push_runtime,
            "a runtime is pushed unless the owner says not"
        );
        assert!(
            !parse_nodes("[nodes]\npush_runtime = false\n")
                .unwrap()
                .push_runtime
        );
        assert!(parse_nodes("[nodes]\npush_runtime = 1\n")
            .unwrap_err()
            .contains("nodes.push_runtime"));
        assert!(c.enabled && c.prefer == Prefer::Github);
        assert_eq!(c.static_hosts, ["studio.local:7425", "127.0.0.1:1"]);
        assert_eq!((c.status_every_s, c.lost_after_polls), (2, 3));
        assert!(c.allows("mason") && !c.allows("star") && !c.allows("other"));
        let s = c.settings_for("studio");
        assert_eq!(
            (s.max_jobs, s.max_die_c, s.on_switch_off),
            (6, 90, OnSwitchOff::Cancel)
        );
        let other = c.settings_for("mini");
        assert_eq!((other.max_jobs, other.max_die_c), (4, 0));
        for (bad, names) in [
            ("[nodes]\nenabled = \"yes\"\n", "nodes.enabled"),
            ("[nodes]\nprefer = \"x\"\n", "nodes.prefer"),
            ("[nodes]\nstatic = [\"nohost\"]\n", "nodes.static"),
            ("[nodes]\nstatus_every_s = 0\n", "nodes.status_every_s"),
            ("[nodes]\nbogus = 1\n", "nodes.bogus"),
            ("[nodes.default]\nmax_jobs = 0\n", "nodes.default.max_jobs"),
            (
                "[nodes.default]\nmax_die_c = 20\n",
                "nodes.default.max_die_c",
            ),
            (
                "[nodes.studio]\non_switch_off = \"x\"\n",
                "nodes.studio.on_switch_off",
            ),
            ("[nodes.studio]\nzzz = 1\n", "nodes.studio.zzz"),
        ] {
            let e = parse_nodes(bad).unwrap_err();
            assert!(e.contains(names) && e.contains("daemon.toml"), "{bad}: {e}");
        }
    }

    #[test]
    fn settings_travel_as_json_and_the_strictest_wins() {
        use super::{NodeSettings, OnSwitchOff};
        let s = NodeSettings {
            max_jobs: 3,
            ..NodeSettings::default()
        };
        assert_eq!(NodeSettings::from_json(&s.to_json()).unwrap(), s);
        assert_eq!(
            NodeSettings::from_json(&serde_json::json!({})).unwrap(),
            NodeSettings::default()
        );
        for bad in [
            serde_json::json!({"max_jobs": 0}),
            serde_json::json!({"max_jobs": "x"}),
            serde_json::json!({"on_switch_off": "later"}),
            serde_json::json!({"nope": 1}),
            serde_json::json!([1]),
        ] {
            assert!(NodeSettings::from_json(&bad).is_err(), "{bad}");
        }
        let a = NodeSettings {
            max_jobs: 4,
            max_other_cpu_pct: 60,
            min_disk_free_gb: 10,
            lease_s: 300,
            max_die_c: 0,
            on_switch_off: OnSwitchOff::Drain,
        };
        let b = NodeSettings {
            max_jobs: 2,
            max_other_cpu_pct: 80,
            min_disk_free_gb: 40,
            lease_s: 120,
            max_die_c: 85,
            on_switch_off: OnSwitchOff::Cancel,
        };
        let m = NodeSettings::strictest(&[a, b]);
        assert_eq!(
            (
                m.max_jobs,
                m.max_other_cpu_pct,
                m.min_disk_free_gb,
                m.lease_s,
                m.max_die_c,
                m.on_switch_off
            ),
            (2, 60, 40, 120, 85, OnSwitchOff::Cancel)
        );
        assert_eq!(NodeSettings::strictest(&[]), NodeSettings::default());
        assert_eq!(NodeSettings::strictest(&[a]), a);
    }

    #[test]
    fn lan_is_off_by_default_and_bad_values_name_the_file() {
        use super::{parse_lan, LanCfg};
        let h = std::path::Path::new("/h");
        assert_eq!(
            super::parse_onboarding("", h).unwrap().roots,
            [std::path::PathBuf::from("/h/git")]
        );
        assert_eq!(
            super::parse_onboarding("[onboarding]\nroots = [\"~\", \"/a\", \"~/x\"]\n", h)
                .unwrap()
                .roots,
            ["/h", "/a", "/h/x"].map(std::path::PathBuf::from)
        );
        assert!(super::parse_onboarding("[onboarding]\nroots = \"x\"\n", h).is_err());
        assert!(super::parse_onboarding("[onboarding]\nfoo = 1\n", h).is_err());
        assert_eq!(parse_lan("").unwrap(), LanCfg::default());
        assert!(!LanCfg::default().enabled);
        assert_eq!(
            parse_lan("[daemon]\nport = 1\n").unwrap(),
            LanCfg::default()
        );
        let l = parse_lan("[lan]\nenabled = true\nport = 9000\n").unwrap();
        assert!(l.enabled && l.port == 9000);
        for bad in [
            "[lan]\nenabled = \"yes\"\n",
            "[lan]\nport = 70000\n",
            "[lan]\nhost = \"x\"\n",
            "[lan]\nhosts = \"x\"\n",
            "[lan]\nhosts = [1]\n",
            "[lan]\nhosts = [\"a b\"]\n",
            "[lan]\nhosts = [\"a&alt=b\"]\n",
            "[lan]\nhosts = [\"\"]\n",
            "[lan]\nbind = []\n",
            "[lan]\nbind = \"0.0.0.0\"\n",
            "[lan]\nbind = [\"not-an-ip\"]\n",
            "lan = 3\n",
        ] {
            let e = parse_lan(bad).unwrap_err();
            assert!(e.starts_with("daemon.toml: "), "{e}");
        }
        let l = parse_lan(
            "[lan]\nhosts = [\"mac.tail.ts.net\", \"100.64.0.1\", \"100.64.0.1\"]\nbind = [\"127.0.0.1\", \"::1\"]\n",
        )
        .unwrap();
        assert_eq!(l.hosts, ["mac.tail.ts.net", "100.64.0.1"]);
        assert_eq!(l.bind.len(), 2);
        assert_eq!(
            LanCfg::default().bind,
            vec![std::net::IpAddr::from([0, 0, 0, 0])]
        );
        assert!(LanCfg::default().hosts.is_empty());
        // The port parser leaves the table alone.
        assert_eq!(
            super::parse_daemon_port("[lan]\nenabled = true\n").unwrap(),
            None
        );
    }

    #[test]
    fn llm_section_still_sets_sessions_for_an_account() {
        let r = crate::sched::parse_resource_cfg("[llm]\nclaude = 2\n").unwrap();
        let accts = crate::providers::default_accounts();
        assert!(crate::providers::check_llm_conflict(&accts, &r.llm).is_ok());
        let b = crate::sched::Broker::new(
            tempfile::tempdir().unwrap().keep(),
            tempfile::tempdir().unwrap().keep().join("limits.kv"),
            &r,
            4,
            std::sync::Arc::new(crate::cpu::FileCpu("/nonexistent".into())),
            &accts,
        )
        .unwrap();
        assert_eq!(b.limit("llm:claude"), Some(2));
    }

    #[test]
    fn the_example_project_file_has_a_tracker_block_that_parses_once_uncommented() {
        let text = include_str!("../../../docs/examples/mason-self.project.toml");
        let off = parse_project("mason", text).unwrap();
        assert!(
            off.services.tracker.is_none(),
            "off until the owner turns it on"
        );
        let on: String = text
            .lines()
            .map(|l| match l.strip_prefix("# ") {
                Some(r)
                    if r.starts_with("[tracker]")
                        || r.starts_with("kind = \"github\"")
                        || r.starts_with("repo =")
                        || r.starts_with("token =")
                        || r.starts_with("label =")
                        || r.starts_with("interval_s =")
                        || r.starts_with("webhook_secret =") =>
                {
                    r.to_string()
                }
                _ => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let t = parse_project("mason", &on)
            .unwrap()
            .services
            .tracker
            .unwrap();
        assert_eq!((t.label.as_str(), t.interval_s), ("mason", 60));
        assert!(t.token.starts_with("env:") && t.webhook_secret.is_some());
        let literal = on.replace("env:MASON_GITHUB_TOKEN", "ghp_literal");
        assert!(
            parse_project("mason", &literal).is_err(),
            "a literal token is refused"
        );
    }

    #[test]
    fn targets_settings_are_read_and_bad_ones_name_the_file() {
        assert_eq!(parse_targets("").unwrap(), TargetsCfg::default());
        let t = parse_targets(
            "[targets]\nprovider = \"fake\"\nfake_file = \"/x/d.json\"\npoll_ms = 500\n",
        )
        .unwrap();
        assert!(t.fake && t.poll_ms == 500);
        assert_eq!(t.fake_file, Some(PathBuf::from("/x/d.json")));
        for bad in [
            "[targets]\nprovider = \"cloud\"\n",
            "[targets]\nprovider = \"fake\"\n",
            "[targets]\npoll_ms = 5\n",
            "[targets]\nwat = 1\n",
        ] {
            let e = parse_targets(bad).unwrap_err();
            assert!(e.starts_with("daemon.toml: "), "{e}");
        }
        // [targets] does not trip the [daemon] port reader.
        assert_eq!(
            parse_daemon_port("[targets]\npoll_ms = 500\n[daemon]\nport = 9\n").unwrap(),
            Some(9)
        );
    }
}
