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
    /// How long after a step run closes (and its hosts exited) the sweep leaves its host
    /// directories in `h/` before moving them to `t/<task>/hosts/` (default a day;
    /// `MASON_HOST_GRACE_MS` sets it for the exit script).
    pub host_grace: Duration,
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
    pub host_bin: PathBuf,
    pub mcp_bin: PathBuf,
    /// 0 picks a free loopback port (written to `daemon.json`).
    pub port: u16,
    /// LAN access (Part 16): off unless the owner turns it on.
    pub lan: LanCfg,
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
    /// A project's fetches must have been failing this long before `fetch:<project>` is raised.
    pub fetch_alert_after: Duration,
    /// A speech command running longer than this for one sentence is killed.
    pub speech_timeout: Duration,
    /// How often alerts are brought up to date and notifications sent.
    pub attention_interval: Duration,
    /// How long a suite waits before trying again after an environmental
    /// failure (no device, the stack would not come up). Charged to no task.
    pub env_retry: Duration,
    /// Identities whose answers count (Part 10). Default: the user running the
    /// daemon.
    pub owner_names: Vec<String>,
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LanCfg {
    pub enabled: bool,
    /// 0 asks the OS for a free port.
    pub port: u16,
}

impl Default for LanCfg {
    fn default() -> Self {
        LanCfg {
            enabled: false,
            port: crate::lan::DEFAULT_LAN_PORT,
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

pub fn parse_daemon_port(text: &str) -> Result<Option<u16>, String> {
    let doc: toml_edit::DocumentMut = text.parse().map_err(|e| format!("daemon.toml: {e}"))?;
    let mut port = None;
    for (key, item) in doc.iter() {
        if key == "lan" || key == "targets" {
            continue; // read by `parse_lan` and `parse_targets`
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
            host_grace: std::env::var("MASON_HOST_GRACE_MS")
                .ok()
                .and_then(|v| v.parse().ok())
                .map_or(Duration::from_secs(24 * 3600), Duration::from_millis),
            retention_days: DEFAULT_RETENTION_DAYS,
            heartbeat: Duration::from_secs(1),
            host_bin,
            mcp_bin,
            port: 0,
            lan: LanCfg::default(),
            stall_threshold: Duration::from_millis(500),
            hold_retry: Duration::from_secs(60),
            debug: false,
            reload_interval: Duration::from_secs(5),
            fetch_interval: Duration::from_secs(60),
            stuck_after: Duration::from_secs(300),
            fetch_alert_after: Duration::from_secs(15 * 60),
            speech_timeout: Duration::from_secs(30),
            attention_interval: Duration::from_secs(5),
            env_retry: Duration::from_secs(30),
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
    pub test_timeout: Duration,
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
    /// A command that reads a task body on stdin and prints `clear` or
    /// `unclear`. Optional, and only consulted when the deterministic check is
    /// unsure. Anything but `clear`, or a failure, means explore.
    pub clarity_classifier: Option<String>,
    /// Opt in to letting top-priority tasks jump the "fewest steps remaining"
    /// term of the scheduling rank (Part 7). Off by default.
    pub priority_jump: bool,
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
    Ok(AgentCfg {
        provider,
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostCfg {
    Github {
        api: String,
        repo: String,
        token: String,
    },
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
    /// Tell infrastructure reds from real ones (`github-actions` only): only infrastructure
    /// is rerun, a real red goes back to the tasks at once. Off by default.
    pub infra_judge: bool,
    /// Log phrases that also mean infrastructure, added to the built-in list.
    pub infra_patterns: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CiKind {
    GithubActions {
        api: String,
        repo: String,
        token: String,
        workflow: Option<String>,
    },
    Command {
        trigger: String,
        status: String,
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
            o => {
                return Err(format!(
                    "{id}: `[host] kind` is `github` or `fake`, not {o:?}"
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
            o => {
                return Err(format!(
                    "{id}: `[ci] kind` is `github-actions`, `jenkins`, `generic` or `command`, not {o:?}"
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
        });
    }
    if let Some(t) = table("tracker") {
        match need(t, "tracker", "kind")?.as_str() {
            "github" => {}
            o => return Err(format!("{id}: `[tracker] kind` is `github` (the local tracker needs no section), not {o:?}")),
        }
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
        out.tracker = Some(TrackerCfg {
            api: api(t),
            repo: need(t, "tracker", "repo")?,
            token: token(t, "tracker")?,
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
        test_timeout: Duration::from_secs(secs("test_timeout_s", 1800)),
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
        task_cache: match s("task_cache").filter(|t| !t.trim().is_empty()) {
            Some(t) => {
                crate::taskcache::check_template(&t).map_err(|e| format!("{id}: {e}"))?;
                Some(t)
            }
            None => None,
        },
        clarity_classifier: s("clarity_classifier").filter(|c| !c.trim().is_empty()),
        card_rewriter: s("card_rewriter").filter(|c| !c.trim().is_empty()),
        priority_jump: doc
            .get("priority_jumps_progress")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        overlay_text: text.to_string(),
        services: parse_services(id, &doc)?,
        self_cfg: parse_self(id, &doc)?,
    })
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
    fn agent_keys_default_to_claude_with_no_ladder_and_no_sandbox() {
        let p = parse_project("p", "path=\"/r\"\nbase=\"m\"\ntest=\"t\"\n").unwrap();
        let a = p.agent();
        assert_eq!(a.provider, Provider::Claude);
        assert!(a.ladder.is_empty() && a.codex_bin.is_none());
        assert_eq!(a.max_retries, 1);
        assert_eq!(a.sandbox, mason_adapters::sandbox::SandboxKind::None);
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
    fn lan_is_off_by_default_and_bad_values_name_the_file() {
        use super::{parse_lan, LanCfg};
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
            "lan = 3\n",
        ] {
            let e = parse_lan(bad).unwrap_err();
            assert!(e.starts_with("daemon.toml: "), "{e}");
        }
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
