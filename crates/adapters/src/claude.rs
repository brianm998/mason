// SPDX-License-Identifier: GPL-3.0-or-later

//! The Claude Code adapter: builds the CLI command, sanitizes its environment,
//! and turns its `stream-json` output into [`AgentEvent`]s.
//!
//! Command construction and stream parsing are pure and unit-tested against
//! captured fixtures. Launching is a thin step on top, so a later stage can
//! run the same command under a step host.

use mason_adapter_api::contract::preamble;
use mason_adapter_api::{
    netguard, AdapterError, AgentAdapter, AgentCaps, AgentEvent, BillingKind, Blocked, BlockedWhy,
    Canceller, Outcome, Overage, OverageState, SessionHandle, SessionSpec, TokenReport, UsageModel,
    UsageSnapshot, Window, WindowSpec, WindowState,
};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Environment variables passed through to the agent. Anything else, notably
/// every git host credential, SSH agent and cloud key, is dropped (Part 6).
/// `ANTHROPIC_*` and `CLAUDE_*` are provider configuration the CLI needs.
const PASS: &[&str] = &[
    "PATH", "HOME", "USER", "LOGNAME", "SHELL", "LANG", "LC_ALL", "TMPDIR", "TERM", "TZ",
];
const PASS_PREFIX: &[&str] = &["LC_", "ANTHROPIC_", "CLAUDE_"];

/// The environment the agent gets: an allowlist from `parent`, then fixed
/// hardening, then the step's own `spec.env`.
pub fn sanitized_env(
    spec: &SessionSpec,
    parent: impl IntoIterator<Item = (String, String)>,
    contract_sock: Option<&Path>,
) -> Vec<(String, String)> {
    sanitized_env_for(spec, parent, contract_sock, PASS_PREFIX)
}

/// [`sanitized_env`] with the provider's own variable prefixes (`OPENAI_`,
/// `CODEX_` for Codex): the same allowlist and hardening, a different provider.
pub fn sanitized_env_for(
    spec: &SessionSpec,
    parent: impl IntoIterator<Item = (String, String)>,
    contract_sock: Option<&Path>,
    prefixes: &[&str],
) -> Vec<(String, String)> {
    let prefixes: Vec<&str> = std::iter::once("LC_")
        .chain(prefixes.iter().copied().filter(|p| *p != "LC_"))
        .collect();
    let mut env: Vec<(String, String)> = parent
        .into_iter()
        .filter(|(k, _)| PASS.contains(&k.as_str()) || prefixes.iter().any(|p| k.starts_with(p)))
        .collect();
    let mut set = |k: &str, v: String| {
        env.retain(|(ek, _)| ek != k);
        env.push((k.to_string(), v));
    };
    // No prompts, no credentials, and git cannot reach any remote.
    set("GIT_TERMINAL_PROMPT", "0".into());
    set("GIT_ASKPASS", "/usr/bin/false".into());
    set("GIT_ALLOW_PROTOCOL", "file".into());
    set("GIT_CONFIG_NOSYSTEM", "1".into());
    set("NO_COLOR", "1".into());
    set("DISABLE_AUTOUPDATER", "1".into());
    // Explicit, never the CLI's default.
    set(
        "MCP_TOOL_TIMEOUT",
        spec.tool_timeout.as_millis().to_string(),
    );
    set("MASON_STEP_KIND", spec.step.name().into());
    if let Some(s) = contract_sock {
        set("MASON_CONTRACT_SOCK", s.display().to_string());
    }
    for (k, v) in &spec.env {
        set(k, v.clone());
    }
    env
}

/// The efforts this provider's CLI accepts.
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Everything needed to launch the CLI, with no side effects.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    /// Written to the CLI's stdin: the prompt, or the resume note.
    pub stdin: String,
}

#[derive(Clone, Debug)]
pub struct ClaudeAdapter {
    /// The `claude` executable (or a scripted stand-in).
    pub bin: PathBuf,
    /// The MCP bridge executable that exposes the step contract.
    pub mcp_bin: PathBuf,
    /// Where per-session MCP config files are written (not the worktree).
    pub state_dir: PathBuf,
    /// The environment to filter. Defaults to this process's.
    pub parent_env: Option<Vec<(String, String)>>,
    /// Run the CLI under a step host (so it survives the controller).
    pub host: Option<crate::hosted::HostLaunch>,
    /// Run the CLI under an OS sandbox (off by default).
    pub sandbox: Option<crate::sandbox::SandboxPolicy>,
}

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

impl ClaudeAdapter {
    pub fn new(
        bin: impl Into<PathBuf>,
        mcp_bin: impl Into<PathBuf>,
        state_dir: impl Into<PathBuf>,
    ) -> Self {
        ClaudeAdapter {
            bin: bin.into(),
            mcp_bin: mcp_bin.into(),
            state_dir: state_dir.into(),
            parent_env: None,
            host: None,
            sandbox: None,
        }
    }

    pub fn with_host(mut self, h: crate::hosted::HostLaunch) -> Self {
        self.host = Some(h);
        self
    }

    fn parent(&self) -> Vec<(String, String)> {
        self.parent_env
            .clone()
            .unwrap_or_else(|| std::env::vars().collect())
    }

    pub fn build_command(
        &self,
        spec: &SessionSpec,
        resume: Option<(&str, &str)>,
    ) -> Result<AgentCommand, AdapterError> {
        let sock = spec.contract.as_ref().and_then(|c| c.sock());
        let env = sanitized_env(spec, self.parent(), sock.as_deref());
        let base_url = env
            .iter()
            .find(|(k, _)| k == "ANTHROPIC_BASE_URL")
            .map(|(_, v)| v.as_str());
        netguard::check_endpoint("ANTHROPIC_BASE_URL", base_url)?;

        let mut args: Vec<String> = [
            "-p",
            "--verbose",
            "--output-format",
            "stream-json",
            "--strict-mcp-config",
        ]
        .map(String::from)
        .to_vec();
        if let Some(m) = &spec.model {
            args.extend(["--model".into(), m.clone()]);
        }
        if let Some(e) = &spec.effort {
            args.extend(["--effort".into(), e.clone()]);
        }
        if let Some((id, _)) = resume {
            args.extend(["--resume".into(), id.into()]);
        }

        let mcp_path = self.write_mcp_config(spec, sock.as_deref())?;
        args.extend(["--mcp-config".into(), mcp_path.display().to_string()]);

        // Built-in tools: read-only steps get no way to change the tree.
        let builtins: &[&str] = if spec.step.read_only() {
            &["Read", "Grep", "Glob"]
        } else {
            &["Read", "Grep", "Glob", "Edit", "Write", "Bash"]
        };
        args.extend(["--tools".into(), builtins.join(",")]);
        let mut allowed: Vec<String> = builtins.iter().map(|s| s.to_string()).collect();
        let offered: Vec<String> = match &spec.tools {
            Some(t) => t.clone(),
            None => spec.step.tools().iter().map(|s| s.to_string()).collect(),
        };
        allowed.extend(offered.iter().map(|t| format!("mcp__mason__{t}")));
        args.push("--allowedTools".into());
        args.extend(allowed);
        // Belt and braces on top of the missing credentials and the git
        // protocol restriction: no remote git and no raw network tools.
        args.push("--disallowedTools".into());
        args.extend(
            [
                "Bash(git push:*)",
                "Bash(git fetch:*)",
                "Bash(git pull:*)",
                "Bash(git clone:*)",
                "Bash(git remote:*)",
                "Bash(curl:*)",
                "Bash(wget:*)",
                "WebFetch",
                "WebSearch",
            ]
            .map(String::from),
        );
        args.extend(["--permission-mode".into(), "dontAsk".into()]);
        // Settings sources are left at the CLI's default, so the owner's own
        // user-level settings and instructions apply to their agents (their choice,
        // docs/decisions/0003). Note that user-level hooks run too.
        args.extend(["--append-system-prompt".into(), preamble(spec.step)]);

        let stdin = match resume {
            Some((_, note)) => note.to_string(),
            None => spec.prompt.clone(),
        };
        Ok(AgentCommand {
            program: self.bin.clone(),
            args,
            env,
            cwd: spec.workdir.clone(),
            stdin,
        })
    }

    fn write_mcp_config(
        &self,
        spec: &SessionSpec,
        sock: Option<&Path>,
    ) -> Result<PathBuf, AdapterError> {
        let n = SESSION_COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = self.state_dir.join(format!("{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(AdapterError::Spawn)?;
        let mut env = json!({"MASON_STEP_KIND": spec.step.name()});
        if let Some(t) = &spec.tools {
            env["MASON_TOOLS"] = json!(t.join(","));
        }
        if let Some(s) = sock {
            env["MASON_CONTRACT_SOCK"] = json!(s.display().to_string());
        }
        let cfg = json!({"mcpServers": {"mason": {
            "command": self.mcp_bin.display().to_string(), "args": [], "env": env}}});
        let path = dir.join("mcp.json");
        std::fs::write(&path, cfg.to_string()).map_err(AdapterError::Spawn)?;
        Ok(path)
    }

    fn launch(
        &self,
        cmd: AgentCommand,
        spec: &SessionSpec,
    ) -> Result<Box<dyn SessionHandle>, AdapterError> {
        let cmd = crate::sandbox::apply(cmd, self.sandbox.as_ref())?;
        launch_process(self.host.as_ref(), cmd, spec, Dialect::Claude)
    }
}

impl AgentAdapter for ClaudeAdapter {
    fn name(&self) -> &'static str {
        "claude-code"
    }

    fn capabilities(&self) -> AgentCaps {
        AgentCaps {
            resume: true,
            mcp: true,
            effort_levels: EFFORT_LEVELS.map(String::from).to_vec(),
            usage_reports: true,
        }
    }

    fn usage_model(&self, billing: BillingKind) -> UsageModel {
        match billing {
            BillingKind::Subscription => UsageModel {
                windows: ["five_hour", "seven_day", "seven_day_opus"]
                    .map(|id| {
                        let (label, len) = claude_window_spec(id);
                        WindowSpec::new(id, &label, len)
                    })
                    .to_vec(),
                utilization: true,
                overage: true,
                spend: false,
            },
            BillingKind::Api => UsageModel {
                windows: vec![],
                utilization: false,
                overage: false,
                spend: true,
            },
        }
    }

    fn start(&self, spec: SessionSpec) -> Result<Box<dyn SessionHandle>, AdapterError> {
        let cmd = self.build_command(&spec, None)?;
        self.launch(cmd, &spec)
    }

    fn resume(
        &self,
        id: &str,
        note: String,
        spec: SessionSpec,
    ) -> Result<Box<dyn SessionHandle>, AdapterError> {
        let cmd = self.build_command(&spec, Some((id, &note)))?;
        self.launch(cmd, &spec)
    }

    fn reattach(
        &self,
        run_key: &str,
        spec: SessionSpec,
    ) -> Result<Option<Box<dyn SessionHandle>>, AdapterError> {
        let Some(h) = &self.host else {
            return Ok(None);
        };
        Ok(crate::hosted::latest_invocation(&h.root, run_key)
            .map(|id| crate::hosted::attach(h, id, &spec)))
    }
}

/// Start the CLI described by `cmd`, directly or under a step host, and follow
/// its output as `dialect`.
pub(crate) fn launch_process(
    host: Option<&crate::hosted::HostLaunch>,
    cmd: AgentCommand,
    spec: &SessionSpec,
    dialect: Dialect,
) -> Result<Box<dyn SessionHandle>, AdapterError> {
    if let Some(h) = host {
        let key = spec
            .run_key
            .as_deref()
            .ok_or_else(|| AdapterError::Other("a hosted launch needs spec.run_key".into()))?;
        let id = crate::hosted::spawn(
            h,
            key,
            &cmd.program,
            &cmd.args,
            &cmd.env,
            &cmd.cwd,
            &cmd.stdin,
        )?;
        return Ok(crate::hosted::attach_as(h, id, spec, dialect));
    }
    let mut c = Command::new(&cmd.program);
    c.args(&cmd.args)
        .current_dir(&cmd.cwd)
        .env_clear()
        .envs(cmd.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = c.spawn().map_err(AdapterError::Spawn)?;
    let pid = child.id();
    if let Some(mut sin) = child.stdin.take() {
        let _ = sin.write_all(cmd.stdin.as_bytes());
    }
    let stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let tail = Arc::new(Mutex::new(String::new()));
    {
        let tail = tail.clone();
        thread::spawn(move || {
            let mut s = String::new();
            let _ = stderr.read_to_string(&mut s);
            let keep: String = s
                .chars()
                .rev()
                .take(2000)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            *tail.lock().unwrap_or_else(|e| e.into_inner()) = keep;
        });
    }
    let (tx, rx) = channel();
    let session_id = Arc::new(Mutex::new(None));
    let cancelled = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));

    // Deadline watchdog: kill the whole process group when time is up.
    if let Some(d) = spec.deadline {
        let (timed_out, done) = (timed_out.clone(), done.clone());
        thread::spawn(move || {
            let end = Instant::now() + d;
            while Instant::now() < end {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            timed_out.store(true, Ordering::SeqCst);
            let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
        });
    }

    let accepted = spec.accepted.clone();
    {
        let (sid, cancelled, timed_out, done) = (
            session_id.clone(),
            cancelled.clone(),
            timed_out.clone(),
            done.clone(),
        );
        thread::spawn(move || {
            let mut st = ParseState::new(dialect);
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                for ev in parse_line(&line, &mut st) {
                    if let Some(id) = &st.session_id {
                        *sid.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());
                    }
                    let _ = tx.send(ev);
                }
            }
            let status = child.wait();
            done.store(true, Ordering::SeqCst);
            let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL); // stragglers
            std::thread::sleep(Duration::from_millis(20)); // let the stderr reader finish
            let detail = tail
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .trim()
                .to_string();
            let outcome = if timed_out.load(Ordering::SeqCst) {
                Outcome::Timeout
            } else if cancelled.load(Ordering::SeqCst) {
                Outcome::NoResult
            } else {
                final_outcome(
                    &st,
                    accepted.is_set(),
                    status.ok().and_then(|s| s.code()),
                    &detail,
                )
            };
            let _ = tx.send(AgentEvent::Finished(outcome));
        });
    }
    Ok(Box::new(ProcessSession {
        rx,
        session_id,
        cancelled,
        pid,
        finished: false,
    }))
}

struct ProcessSession {
    rx: Receiver<AgentEvent>,
    session_id: Arc<Mutex<Option<String>>>,
    cancelled: Arc<AtomicBool>,
    pid: u32,
    finished: bool,
}

impl SessionHandle for ProcessSession {
    fn session_id(&self) -> Option<String> {
        self.session_id
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn next_event(&mut self) -> Option<AgentEvent> {
        if self.finished {
            return None;
        }
        loop {
            match self.rx.recv_timeout(Duration::from_secs(3600)) {
                Ok(e) => {
                    if matches!(e, AgentEvent::Finished(_)) {
                        self.finished = true;
                    }
                    return Some(e);
                }
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => {
                    self.finished = true;
                    return Some(AgentEvent::Finished(Outcome::Crash {
                        detail: "event stream lost".into(),
                    }));
                }
            }
        }
    }

    fn canceller(&self) -> Canceller {
        let (c, pid) = (self.cancelled.clone(), self.pid);
        Arc::new(move || {
            c.store(true, Ordering::SeqCst);
            let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
        })
    }
}

/// Which CLI's output a stream is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Dialect {
    #[default]
    Claude,
    Codex,
}

/// What the parser has learned so far from one invocation's stream.
#[derive(Default, Debug)]
pub struct ParseState {
    pub dialect: Dialect,
    pub session_id: Option<String>,
    pub result: Option<ResultInfo>,
    /// The latest usage snapshot the stream gave, for classifying a failed result.
    pub usage: Option<UsageSnapshot>,
    /// From the stream's own `apiKeySource`: a key means pay-per-token.
    pub billing: BillingKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultInfo {
    pub is_error: bool,
    pub text: String,
    pub api_error_status: Option<u32>,
}

impl ParseState {
    pub fn new(dialect: Dialect) -> Self {
        ParseState {
            dialect,
            ..Default::default()
        }
    }
}

pub(crate) fn num(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

pub(crate) fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n).collect::<String>())
    }
}

/// The windows Claude names, with their labels and lengths.
const CLAUDE_WINDOWS: &[(&str, &str, u64)] = &[
    ("five_hour", "5-hour", 5 * 3600),
    ("seven_day", "Weekly", 7 * 86400),
];

fn claude_window_spec(id: &str) -> (String, Option<u64>) {
    match id {
        "seven_day_opus" => ("Weekly (Opus)".into(), Some(7 * 86400)),
        _ => CLAUDE_WINDOWS
            .iter()
            .find(|(i, _, _)| *i == id)
            .map(|(_, l, n)| (l.to_string(), Some(*n)))
            // An id not known yet passes through, labelled with itself.
            .unwrap_or((id.to_string(), None)),
    }
}

fn window_state(status: Option<&str>) -> WindowState {
    match status {
        Some("allowed") | None => WindowState::Ok,
        Some("allowed_warning") => WindowState::Warning,
        // `rejected`, and anything we do not know: when unsure, blocked.
        Some(_) => WindowState::Blocked,
    }
}

fn claude_window(id: &str, status: Option<&str>, w: &Value) -> Window {
    let (label, length_secs) = claude_window_spec(id);
    Window {
        id: id.into(),
        label,
        length_secs,
        // Only what the stream reported, and only a fraction.
        utilization: w["utilization"]
            .as_f64()
            .filter(|u| (0.0..=1.0).contains(u)),
        resets_at: num(w, "resetsAt"),
        state: window_state(status.or_else(|| w["status"].as_str())),
    }
}

/// Read one `rate_limit_info` object: every window, the overage fields, and
/// from them whether new work is refused (ADR 0043).
pub fn usage_from_limit_info(i: &Value) -> UsageSnapshot {
    let top_status = i["status"].as_str();
    let top_type = i["rateLimitType"].as_str();
    let mut windows: Vec<Window> = i["unifiedWindows"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(id, w)| claude_window(id, None, w))
        .collect();
    // The window named at the top level, when `unifiedWindows` lacks it.
    if let Some(t) = top_type {
        if !windows.iter().any(|w| w.id == t) {
            windows.push(claude_window(t, top_status, i));
            if let Some(w) = windows.last_mut() {
                w.utilization = None;
            }
        }
    }
    let using = i["isUsingOverage"].as_bool();
    let o_status = i["overageStatus"].as_str();
    let o_reason = i["overageDisabledReason"].as_str();
    let state = match (using, o_status) {
        (Some(true), Some("rejected")) => OverageState::UsedUp,
        (Some(true), _) => OverageState::InUse,
        (_, Some("rejected")) => match o_reason {
            Some(r) => OverageState::Disabled(r.to_string()),
            None => OverageState::Off,
        },
        (None, None) if o_reason.is_none() => OverageState::NotOffered,
        (_, _) => OverageState::Off,
    };
    let overage = Overage {
        state,
        resets_at: num(i, "overageResetsAt"),
    };
    let blocked_windows: Vec<&Window> = windows
        .iter()
        .filter(|w| w.state == WindowState::Blocked)
        .collect();
    // It stays blocked until the last blocked window clears.
    let latest = blocked_windows
        .iter()
        .max_by_key(|w| w.resets_at.unwrap_or(0))
        .copied();
    let blocked = match (&overage.state, latest) {
        (OverageState::InUse, _) => None,
        (OverageState::UsedUp, Some(w)) => Some(Blocked {
            why: BlockedWhy::OverageUsedUp,
            until: overage.resets_at.or(w.resets_at),
        }),
        (_, Some(w)) => Some(Blocked {
            why: BlockedWhy::Window(w.id.clone()),
            until: w.resets_at,
        }),
        // Refused at the top level with no window to name.
        (_, None) if window_state(top_status) == WindowState::Blocked => Some(Blocked {
            why: BlockedWhy::Unreported(format!("status {}", top_status.unwrap_or("?"))),
            until: num(i, "resetsAt"),
        }),
        _ => None,
    };
    UsageSnapshot {
        windows,
        overage,
        blocked,
        ..Default::default()
    }
}

/// Parse one `stream-json` line. Unknown or malformed lines yield nothing.
pub fn parse_line(line: &str, st: &mut ParseState) -> Vec<AgentEvent> {
    if st.dialect == Dialect::Codex {
        return crate::codex::parse_line(line, st);
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return vec![];
    };
    let mut out = vec![];
    match (v["type"].as_str(), v["subtype"].as_str()) {
        (Some("system"), Some("init")) => {
            st.session_id = v["session_id"].as_str().map(String::from);
            if v["apiKeySource"].as_str().is_some_and(|k| k != "none") {
                st.billing = BillingKind::Api;
            }
        }
        (Some("system"), Some("api_retry")) => out.push(AgentEvent::ApiRetry {
            attempt: num(&v, "attempt").unwrap_or(0) as u32,
            status: num(&v, "error_status").map(|n| n as u32),
            delay_ms: num(&v, "retry_delay_ms"),
        }),
        (Some("assistant"), _) => {
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = block["text"].as_str() {
                            out.push(AgentEvent::Text(t.to_string()));
                        }
                    }
                    Some("tool_use") => out.push(AgentEvent::ToolCall {
                        name: block["name"].as_str().unwrap_or("?").to_string(),
                        summary: clip(&block["input"].to_string(), 200),
                    }),
                    _ => {}
                }
            }
        }
        (Some("rate_limit_event"), _) => {
            let mut snap = usage_from_limit_info(&v["rate_limit_info"]);
            snap.session_id = v["session_id"]
                .as_str()
                .map(String::from)
                .or_else(|| st.session_id.clone());
            st.usage = Some(snap.clone());
            out.push(AgentEvent::Usage(snap));
        }
        (Some("result"), _) => {
            let u = &v["usage"];
            out.push(AgentEvent::Tokens(TokenReport {
                input: num(u, "input_tokens"),
                output: num(u, "output_tokens"),
                cache_read: num(u, "cache_read_input_tokens"),
                cache_create: num(u, "cache_creation_input_tokens"),
                cost_usd: v["total_cost_usd"].as_f64(),
            }));
            if st.session_id.is_none() {
                st.session_id = v["session_id"].as_str().map(String::from);
            }
            // Spend belongs to an api account; on a plan the figure is notional.
            let spend = (st.billing == BillingKind::Api)
                .then(|| v["total_cost_usd"].as_f64())
                .flatten();
            let refused = num(&v, "api_error_status") == Some(429);
            let mut snap = st.usage.clone().unwrap_or_default();
            let mut changed = false;
            if spend.is_some() {
                snap.spend_usd = spend;
                changed = true;
            }
            // A 429 no window explains is a rate limit that clears in minutes.
            if refused
                && snap.blocked.is_none()
                && !snap.windows.iter().any(|w| w.state == WindowState::Blocked)
            {
                snap.blocked = Some(Blocked {
                    why: BlockedWhy::RateLimit,
                    until: None,
                });
                changed = true;
            }
            if changed {
                snap.session_id = st.session_id.clone();
                st.usage = Some(snap.clone());
                out.push(AgentEvent::Usage(snap));
            }
            st.result = Some(ResultInfo {
                is_error: v["is_error"].as_bool().unwrap_or(false),
                text: v["result"].as_str().unwrap_or_default().to_string(),
                api_error_status: num(&v, "api_error_status").map(|n| n as u32),
            });
        }
        _ => {}
    }
    out
}

/// Decide how an invocation ended, from its stream and exit.
pub fn decide(st: &ParseState, accepted: bool, exit_code: Option<i32>, stderr: &str) -> Outcome {
    final_outcome(st, accepted, exit_code, stderr)
}

/// See [`decide`].
pub fn final_outcome(
    st: &ParseState,
    accepted: bool,
    exit_code: Option<i32>,
    stderr: &str,
) -> Outcome {
    let Some(r) = &st.result else {
        let detail = match exit_code {
            Some(c) => format!(
                "the CLI exited with status {c} and no result; stderr: {}",
                clip(stderr, 500)
            ),
            None => format!(
                "the CLI was killed and gave no result; stderr: {}",
                clip(stderr, 500)
            ),
        };
        return Outcome::Crash { detail };
    };
    if !r.is_error {
        return if accepted {
            Outcome::Accepted
        } else {
            Outcome::NoResult
        };
    }
    let t = r.text.to_lowercase();
    let limited = r.api_error_status == Some(429)
        || st.usage.as_ref().is_some_and(|u| {
            u.blocked.is_some() || u.windows.iter().any(|w| w.state == WindowState::Blocked)
        })
        || (t.contains("limit")
            && (t.contains("usage") || t.contains("hit your") || t.contains("reached")));
    if limited {
        let resets_at = r
            .text
            .split_once("resets")
            .map(|(_, rest)| rest.trim().to_string())
            .or_else(|| {
                let u = st.usage.as_ref()?;
                u.blocked
                    .as_ref()
                    .and_then(|b| b.until)
                    .or_else(|| {
                        u.windows
                            .iter()
                            .filter(|w| w.state == WindowState::Blocked)
                            .filter_map(|w| w.resets_at)
                            .max()
                    })
                    .map(|e| format!("epoch {e}"))
            });
        return Outcome::UsageLimit { resets_at };
    }
    let down = r.api_error_status.is_some_and(|s| s >= 500)
        || t.contains("overloaded")
        || t.contains("api error: 5")
        || t.contains("service unavailable");
    if down {
        return Outcome::ProviderDown {
            detail: clip(&r.text, 300),
        };
    }
    Outcome::Crash {
        detail: clip(&r.text, 300),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mason_adapter_api::StepKind;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/fixtures/claude/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    fn parse(name: &str) -> (Vec<AgentEvent>, ParseState) {
        let mut st = ParseState::default();
        let ev = fixture(name)
            .lines()
            .flat_map(|l| parse_line(l, &mut st))
            .collect();
        (ev, st)
    }

    #[test]
    fn real_plain_run_parses() {
        let (ev, st) = parse("real_plain.jsonl");
        assert!(st.session_id.is_some());
        assert!(
            ev.iter()
                .any(|e| matches!(e, AgentEvent::Text(t) if t == "pong")),
            "{ev:?}"
        );
        let tokens = ev
            .iter()
            .find_map(|e| {
                if let AgentEvent::Tokens(t) = e {
                    Some(t)
                } else {
                    None
                }
            })
            .unwrap();
        assert!(
            tokens.output.is_some() && tokens.cache_read.is_some() && tokens.cost_usd.is_some()
        );
        assert_eq!(
            final_outcome(&st, false, Some(0), ""),
            Outcome::NoResult,
            "no submission"
        );
        assert_eq!(final_outcome(&st, true, Some(0), ""), Outcome::Accepted);
    }

    #[test]
    fn real_tool_run_reports_tool_calls_and_honest_rate_limits() {
        let (ev, _) = parse("real_tool_suspend.jsonl");
        assert!(ev.iter().any(
            |e| matches!(e, AgentEvent::ToolCall { name, .. } if name == "mcp__spike__run_check")
        ));
        let u = ev
            .iter()
            .find_map(|e| {
                if let AgentEvent::Usage(u) = e {
                    Some(u)
                } else {
                    None
                }
            })
            .unwrap();
        let w = u.window("five_hour").unwrap();
        assert_eq!(w.state, WindowState::Ok);
        assert!(w.resets_at.is_some());
        assert!(
            w.utilization.is_some(),
            "reported by the stream, so passed on"
        );
    }

    #[test]
    fn synthetic_usage_limit_is_a_hold_with_a_reset_time() {
        let (ev, st) = parse("synthetic_usage_limit.jsonl");
        assert!(
            matches!(ev.iter().find(|e| matches!(e, AgentEvent::Usage(_))), Some(AgentEvent::Usage(u)) if u.window("five_hour").is_some_and(|w| w.state == WindowState::Blocked) && u.windows[0].utilization.is_none())
        );
        assert_eq!(
            final_outcome(&st, false, Some(1), ""),
            Outcome::UsageLimit {
                resets_at: Some("3pm".into())
            }
        );
    }

    #[test]
    fn synthetic_outage_reports_retries_then_provider_down() {
        let (ev, st) = parse("synthetic_provider_down.jsonl");
        let retries: Vec<_> = ev
            .iter()
            .filter(|e| matches!(e, AgentEvent::ApiRetry { .. }))
            .collect();
        assert_eq!(retries.len(), 2);
        assert!(matches!(
            final_outcome(&st, false, Some(1), ""),
            Outcome::ProviderDown { .. }
        ));
    }

    #[test]
    fn a_logged_out_cli_is_a_crash_not_a_hold() {
        let (_, st) = parse("synthetic_not_logged_in.jsonl");
        match final_outcome(&st, false, Some(1), "") {
            Outcome::Crash { detail } => assert!(detail.contains("Not logged in")),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn no_result_event_is_a_crash_with_stderr() {
        let st = ParseState::default();
        match final_outcome(&st, false, Some(2), "boom: segfault") {
            Outcome::Crash { detail } => {
                assert!(detail.contains("status 2") && detail.contains("segfault"))
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let mut st = ParseState::default();
        for l in [
            "",
            "not json",
            "{",
            "[]",
            "{\"type\":42}",
            "{\"type\":\"mystery\"}",
        ] {
            assert!(parse_line(l, &mut st).is_empty(), "{l:?}");
        }
    }

    fn spec(step: StepKind) -> SessionSpec {
        let mut s = SessionSpec::new(step, "do the thing", "/work");
        s.env = vec![("EXTRA".into(), "1".into())];
        s
    }

    fn adapter(parent: &[(&str, &str)]) -> ClaudeAdapter {
        let mut a = ClaudeAdapter::new(
            "/bin/claude",
            "/bin/mason-mcp",
            std::env::temp_dir().join("mason-adapter-test"),
        );
        a.parent_env = Some(
            parent
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        );
        a
    }

    #[test]
    fn environment_is_an_allowlist_with_hardening() {
        let parent = [
            ("PATH", "/usr/bin"),
            ("HOME", "/h"),
            ("LC_CTYPE", "en"),
            ("ANTHROPIC_API_KEY", "k"),
            ("GITHUB_TOKEN", "ghp_x"),
            ("GH_TOKEN", "x"),
            ("SSH_AUTH_SOCK", "/s"),
            ("AWS_SECRET_ACCESS_KEY", "x"),
            ("GIT_ASKPASS", "/real/askpass"),
            ("NPM_TOKEN", "x"),
            ("MCP_TOOL_TIMEOUT", "1"),
        ];
        let env = sanitized_env(
            &spec(StepKind::Develop),
            parent.map(|(k, v)| (k.to_string(), v.to_string())),
            None,
        );
        let get = |k: &str| env.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.as_str());
        for dropped in [
            "GITHUB_TOKEN",
            "GH_TOKEN",
            "SSH_AUTH_SOCK",
            "AWS_SECRET_ACCESS_KEY",
            "NPM_TOKEN",
        ] {
            assert_eq!(get(dropped), None, "{dropped} must not reach the agent");
        }
        assert_eq!(get("ANTHROPIC_API_KEY"), Some("k"));
        assert_eq!(
            get("GIT_ASKPASS"),
            Some("/usr/bin/false"),
            "overridden, not inherited"
        );
        assert_eq!(get("GIT_ALLOW_PROTOCOL"), Some("file"));
        assert_eq!(get("GIT_TERMINAL_PROMPT"), Some("0"));
        assert_eq!(
            get("MCP_TOOL_TIMEOUT"),
            Some("600000"),
            "explicit, not inherited and not the default"
        );
        assert_eq!(get("EXTRA"), Some("1"));
        assert_eq!(get("MASON_STEP_KIND"), Some("develop"));
        assert_eq!(env.iter().filter(|(k, _)| k == "GIT_ASKPASS").count(), 1);
    }

    #[test]
    fn read_only_steps_get_read_only_tools_and_no_remote_git() {
        let a = adapter(&[
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:1"),
        ]);
        let c = a.build_command(&spec(StepKind::Review), None).unwrap();
        let tools_at = c.args.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(c.args[tools_at + 1], "Read,Grep,Glob");
        let joined = c.args.join(" ");
        assert!(
            joined.contains("mcp__mason__submit_review")
                && !joined.contains("mcp__mason__submit_change")
        );
        assert!(!joined.contains("Edit") || joined.contains("--disallowedTools"));
        let dis = c
            .args
            .iter()
            .position(|x| x == "--disallowedTools")
            .unwrap();
        for banned in ["Bash(git push:*)", "Bash(git fetch:*)", "WebFetch"] {
            assert!(c.args[dis..].contains(&banned.to_string()), "{banned}");
        }
        let c = a.build_command(&spec(StepKind::Develop), None).unwrap();
        let tools_at = c.args.iter().position(|x| x == "--tools").unwrap();
        assert!(c.args[tools_at + 1].contains("Edit") && c.args[tools_at + 1].contains("Bash"));
        assert!(c.args.join(" ").contains("mcp__mason__run_check"));
    }

    #[test]
    fn settings_sources_are_left_at_the_cli_default() {
        let a = adapter(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")]);
        for step in [StepKind::Explore, StepKind::Develop, StepKind::Review] {
            let c = a.build_command(&spec(step), None).unwrap();
            assert!(
                !c.args.iter().any(|x| x == "--setting-sources"),
                "the owner's user settings apply"
            );
        }
    }

    #[test]
    fn a_definitions_tool_list_narrows_what_the_agent_is_allowed() {
        let a = adapter(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")]);
        let mut sp = spec(StepKind::Develop);
        sp.tools = Some(vec!["submit_change".into(), "report_progress".into()]);
        let c = a.build_command(&sp, None).unwrap();
        let joined = c.args.join(" ");
        assert!(
            joined.contains("mcp__mason__submit_change")
                && joined.contains("mcp__mason__report_progress")
        );
        assert!(
            !joined.contains("mcp__mason__run_check") && !joined.contains("mcp__mason__ask"),
            "{joined}"
        );
        let cfg = std::fs::read_to_string(
            c.args[c.args.iter().position(|x| x == "--mcp-config").unwrap() + 1].clone(),
        )
        .unwrap();
        assert!(
            cfg.contains("MASON_TOOLS") && cfg.contains("submit_change,report_progress"),
            "{cfg}"
        );
    }

    #[test]
    fn the_preamble_is_always_appended() {
        let a = adapter(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")]);
        let c = a.build_command(&spec(StepKind::Explore), None).unwrap();
        let at = c
            .args
            .iter()
            .position(|x| x == "--append-system-prompt")
            .unwrap();
        let p = &c.args[at + 1];
        assert!(
            p.contains("No human is watching")
                && p.contains("submit_plan")
                && p.contains("read-only step")
        );
    }

    #[test]
    fn resume_sends_the_note_and_not_the_original_prompt() {
        let a = adapter(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")]);
        let sp = spec(StepKind::Develop);
        let first = a.build_command(&sp, None).unwrap();
        assert_eq!(first.stdin, "do the thing");
        assert!(!first.args.contains(&"--resume".to_string()));
        let again = a
            .build_command(&sp, Some(("sess-1", "Check finished: green.")))
            .unwrap();
        assert_eq!(again.stdin, "Check finished: green.");
        let at = again.args.iter().position(|x| x == "--resume").unwrap();
        assert_eq!(again.args[at + 1], "sess-1");
    }
}
