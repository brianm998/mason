// SPDX-License-Identifier: GPL-3.0-or-later

//! The Claude Code adapter: builds the CLI command, sanitizes its environment,
//! and turns its `stream-json` output into [`AgentEvent`]s.
//!
//! Command construction and stream parsing are pure and unit-tested against
//! captured fixtures. Launching is a thin step on top, so a later stage can
//! run the same command under a step host.

use crate::skills::{self, Skills};
use mason_adapter_api::contract::preamble;
use mason_adapter_api::{
    netguard, AdapterError, AgentAdapter, AgentCaps, AgentEvent, BillingKind, Blocked, BlockedWhy,
    Canceller, Outcome, Overage, OverageState, PlanItem, PlanStatus, SessionHandle, SessionSpec,
    TokenReport, UsageModel, UsageSnapshot, Window, WindowSpec, WindowState,
};
use mason_platform::process::{signal_group, spawn_group, Signal};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
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

/// The variables that say who the agent is. When the env file sets one of them, none
/// of them is inherited: an inherited `ANTHROPIC_API_KEY` would otherwise outrank the
/// file's OAuth token.
const CREDENTIALS: &[&str] = &[
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDE_CODE_OAUTH_TOKEN",
];

/// The `ANTHROPIC_*` and `CLAUDE_*` settings of an env file: `NAME=value` lines, with
/// an optional `export` and optional quotes. Comments, empty values and other names
/// are skipped. This is not a shell: nothing is expanded.
fn parse_env_file(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for line in text.lines() {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if !(k.starts_with("ANTHROPIC_") || k.starts_with("CLAUDE_"))
            || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            continue;
        }
        let quoted = ['"', '\''].into_iter().find_map(|q| {
            let rest = v.strip_prefix(q)?;
            rest.find(q).map(|end| &rest[..end])
        });
        let v = match quoted {
            Some(v) => v,
            None => v.split_whitespace().next().unwrap_or(""),
        };
        if v.is_empty() {
            continue;
        }
        out.retain(|(ek, _)| ek != k);
        out.push((k.to_string(), v.to_string()));
    }
    out
}

/// `parent` with the settings of the env file at `path` laid over it. No file leaves
/// `parent` alone, so the CLI uses its own login.
fn with_env_file(
    mut parent: Vec<(String, String)>,
    path: &Path,
) -> Result<Vec<(String, String)>, AdapterError> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(parent),
        Err(e) => {
            return Err(AdapterError::Other(format!(
                "cannot read {}: {e}",
                path.display()
            )))
        }
    };
    let file = parse_env_file(&text);
    let replaces_login = file.iter().any(|(k, _)| CREDENTIALS.contains(&k.as_str()));
    parent.retain(|(k, _)| {
        !(file.iter().any(|(f, _)| f == k) || replaces_login && CREDENTIALS.contains(&k.as_str()))
    });
    parent.extend(file);
    Ok(parent)
}

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
    // The CLI also aborts a tool that is silent for 30 minutes, whatever the
    // timeout above says, and the check it gave up on goes on running.
    set(
        "CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT",
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

/// The owner's Claude configuration directory, as the session will see it.
fn claude_config_dir(env: &[(String, String)]) -> Option<PathBuf> {
    let get = |k: &str| {
        env.iter()
            .find(|(ek, v)| ek == k && !v.is_empty())
            .map(|(_, v)| PathBuf::from(v))
    };
    get("CLAUDE_CONFIG_DIR").or_else(|| get("HOME").map(|h| h.join(".claude")))
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
    /// A `claude.env` holding the credentials, read again for every session that
    /// starts or resumes, so a renewed token needs no restart.
    pub env_file: Option<PathBuf>,
    /// Run the CLI under a step host (so it survives the controller).
    pub host: Option<crate::hosted::HostLaunch>,
    /// Run the CLI under an OS sandbox (off by default).
    pub sandbox: Option<crate::sandbox::SandboxPolicy>,
    /// The skills a session may use (s1153): the project's, and whether the owner's.
    pub skills: Skills,
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
            env_file: None,
            host: None,
            sandbox: None,
            skills: Skills::default(),
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
        let mut parent = self.parent();
        if let Some(f) = &self.env_file {
            parent = with_env_file(parent, f)?;
        }
        let mut env = sanitized_env(spec, parent, sock.as_deref());
        // What the command guard lets a recursive `rm` reach (s1107). Set last, so a
        // project's agent_env cannot widen it.
        for (k, dir) in [
            ("MASON_GUARD_WORKTREE", &spec.workdir),
            ("MASON_GUARD_STATE", &self.state_dir),
        ] {
            env.retain(|(ek, _)| ek != k);
            env.push((k.to_string(), dir.display().to_string()));
        }
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
            // Each message then ends with a `message_delta` holding its true output
            // figure; without it the stream's output count stays far too low until
            // the final `result`. The extra `stream_event` lines are kept out of the
            // output counts (`mason_core::progress`).
            "--include-partial-messages",
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
        // Exported into this session's own directory, from what the daemon read on
        // the base commit (s1153).
        let skills = skills::prepare(
            &self.skills,
            mcp_path.parent().unwrap_or(&self.state_dir),
            &spec.workdir,
            claude_config_dir(&env).as_deref(),
        )
        .map_err(AdapterError::Spawn)?;
        if let Some(dir) = &skills.plugin_dir {
            args.extend(["--plugin-dir".into(), dir.display().to_string()]);
        }
        // Added to the project's and the owner's settings, not in place of them.
        let settings = self.write_settings(&mcp_path, &skills)?;
        args.extend(["--settings".into(), settings.display().to_string()]);

        // Built-in tools: read-only steps get no way to change the tree.
        let builtins: &[&str] = if spec.step.read_only() {
            &["Read", "Grep", "Glob", "TodoWrite", "Skill"]
        } else {
            &[
                "Read",
                "Grep",
                "Glob",
                "Edit",
                "Write",
                "Bash",
                "TodoWrite",
                "Skill",
            ]
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

    /// The settings file beside `mcp.json`. It puts the command guard (`mason hook
    /// guard`, s1107) in front of every Bash call and the skill guard (`mason hook
    /// skill`, s1153) in front of every Skill call, and carries what `skills` turns off.
    /// `mason` is the binary installed next to the MCP bridge. A guard that cannot run
    /// refuses the call (exit 2): any other failure of a hook lets it through.
    fn write_settings(
        &self,
        mcp_path: &Path,
        skills: &skills::Prepared,
    ) -> Result<PathBuf, AdapterError> {
        let quote = |p: &Path| format!("'{}'", p.display().to_string().replace('\'', "'\\''"));
        let mason = quote(
            &self
                .mcp_bin
                .with_file_name(format!("mason{}", std::env::consts::EXE_SUFFIX)),
        );
        let policy = mcp_path.with_file_name("skills.json");
        std::fs::write(&policy, skills.policy.to_json().to_string())
            .map_err(AdapterError::Spawn)?;
        let guard = |args: String, what: &str, tool: &str| {
            format!(
                "{mason} hook {args} || {{ echo 'MASON: the {what} guard could not run, so {tool} \
                 is refused' >&2; exit 2; }}"
            )
        };
        let hook = |matcher: &str, command: String| json!({"matcher": matcher, "hooks": [{"type": "command", "command": command}]});
        let mut settings = Value::Object(skills.settings.clone());
        settings["hooks"] = json!({"PreToolUse": [
            hook("Bash", guard("guard".into(), "command", "Bash")),
            hook(
                "Skill",
                guard(format!("skill --policy {}", quote(&policy)), "skill", "Skill"),
            ),
            // The write-time rules (s1152). Unlike the guards it fails open.
            hook("Edit|Write|MultiEdit", format!("{mason} hook edit")),
        ]});
        let path = mcp_path.with_file_name("settings.json");
        std::fs::write(&path, settings.to_string()).map_err(AdapterError::Spawn)?;
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
            plan_reports: true,
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
        .stderr(Stdio::piped());
    let mut child = spawn_group(&mut c).map_err(AdapterError::Spawn)?;
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
        let extra = spec.deadline_extra.clone();
        thread::spawn(move || {
            let end = Instant::now() + d;
            while Instant::now() < end + extra.get() {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            timed_out.store(true, Ordering::SeqCst);
            let _ = signal_group(pid, Signal::Kill);
        });
    }

    let accepted = spec.accepted.clone();
    let on_tokens = spec.on_tokens.clone();
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
                let evs = parse_line(&line, &mut st);
                if let (Some(f), Some(t)) = (&on_tokens, st.take_live()) {
                    f("", &t);
                }
                for ev in evs {
                    if let Some(id) = &st.session_id {
                        *sid.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());
                    }
                    let _ = tx.send(ev);
                }
            }
            let status = child.wait();
            done.store(true, Ordering::SeqCst);
            let _ = signal_group(pid, Signal::Kill); // stragglers
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
            let _ = signal_group(pid, Signal::Kill);
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
    /// Figures of the messages this invocation has finished.
    closed: Tally,
    /// The message in progress: `message_start`, replaced by its `message_delta`.
    open: Option<Tally>,
    /// The invocation's cumulative figures when they last changed, taken by
    /// whoever reports them live ([`ParseState::take_live`]).
    live: Option<TokenReport>,
    last_live: Option<TokenReport>,
    context: ContextTrack,
    /// Assistant messages seen so far, for the turn of a file edit.
    turns: u32,
}

/// What is known of the context window's fill.
#[derive(Default, Debug)]
struct ContextTrack {
    /// The main conversation's model, from `system/init`.
    model: String,
    /// The window the stream stated (`system/init` or a `result`).
    reported: Option<u64>,
    /// The input of the latest message; `None` before one, and after a compaction.
    used: Option<u64>,
    /// The `(used, window)` last sent.
    sent: Option<(u64, u64)>,
}

impl ContextTrack {
    fn window(&self, used: u64) -> u64 {
        if let Some(w) = self.reported {
            return w;
        }
        let w = known_window(&self.model);
        // More in use than the assumed window holds: it is the large one.
        if used > w {
            LARGE_WINDOW
        } else {
            w
        }
    }

    /// The event for the figures as they stand, if they moved by a percentage point
    /// or the window changed since the last one sent.
    fn event(&mut self) -> Option<AgentEvent> {
        let used = self.used?;
        let window = self.window(used);
        let moved = self.sent.is_none_or(|(u, w)| {
            w != window || u128::from(u.abs_diff(used)) * 100 >= u128::from(window)
        });
        moved.then(|| {
            self.sent = Some((used, window));
            AgentEvent::Context { used, window }
        })
    }

    /// A message of the main conversation said what its input was.
    fn message(&mut self, usage: &Value) -> Option<AgentEvent> {
        let used: u64 = [
            "input_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ]
        .iter()
        .filter_map(|k| num(usage, k))
        .sum();
        if used == 0 {
            return None;
        }
        self.used = Some(used);
        self.event()
    }

    /// A `result` line's `modelUsage.<model>.contextWindow`: the main model's entry, or
    /// else the one that read the most.
    fn result(&mut self, model_usage: &Value) -> Option<AgentEvent> {
        let entries = model_usage.as_object()?;
        let read = |v: &Value| {
            [
                "inputTokens",
                "cacheReadInputTokens",
                "cacheCreationInputTokens",
            ]
            .iter()
            .filter_map(|k| num(v, k))
            .sum::<u64>()
        };
        let main = entries
            .iter()
            .find(|(k, _)| !self.model.is_empty() && k.starts_with(&self.model))
            .or_else(|| entries.iter().max_by_key(|(_, v)| read(v)))?;
        self.reported = Some(num(main.1, "contextWindow").filter(|w| *w > 0)?);
        self.event()
    }
}

const SMALL_WINDOW: u64 = 200_000;
const LARGE_WINDOW: u64 = 1_000_000;

/// The window assumed for a model until the stream states it. Only Haiku 5.5's was seen
/// in a capture; a `[1m]` name asks for the large one; the rest get the small one.
fn known_window(model: &str) -> u64 {
    if model.contains("[1m]") || model.starts_with("claude-haiku-5-5") {
        LARGE_WINDOW
    } else {
        SMALL_WINDOW
    }
}

/// A string cut to at most `FILE_EDIT_TEXT_MAX` bytes on a character boundary.
fn capped(s: &str, truncated: &mut bool) -> String {
    let max = mason_adapter_api::FILE_EDIT_TEXT_MAX;
    if s.len() <= max {
        return s.to_string();
    }
    *truncated = true;
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// The file edits a `tool_use` block makes: one per `Edit`, one per entry of a `MultiEdit`,
/// one per `Write`. Any other tool makes none.
fn file_edits(name: &str, input: &Value, turn: u32) -> Vec<AgentEvent> {
    let Some(path) = input["file_path"].as_str() else {
        return vec![];
    };
    let one = |old: Option<&str>, new: &str| {
        let mut truncated = false;
        let old = old.map(|o| capped(o, &mut truncated));
        let new = capped(new, &mut truncated);
        AgentEvent::FileEdit {
            tool: name.to_string(),
            path: path.to_string(),
            old,
            new,
            truncated,
            turn,
        }
    };
    match name {
        "Edit" => vec![one(
            Some(input["old_string"].as_str().unwrap_or("")),
            input["new_string"].as_str().unwrap_or(""),
        )],
        "MultiEdit" => input["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|e| {
                one(
                    Some(e["old_string"].as_str().unwrap_or("")),
                    e["new_string"].as_str().unwrap_or(""),
                )
            })
            .collect(),
        "Write" => vec![one(None, input["content"].as_str().unwrap_or(""))],
        _ => vec![],
    }
}

/// input, output, cache read, cache write.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Tally([u64; 4]);

impl Tally {
    fn of(u: &Value) -> Tally {
        Tally([
            num(u, "input_tokens").unwrap_or(0),
            num(u, "output_tokens").unwrap_or(0),
            num(u, "cache_read_input_tokens").unwrap_or(0),
            num(u, "cache_creation_input_tokens").unwrap_or(0),
        ])
    }
    /// `self` with the figures `u` states replacing the ones it had.
    fn replaced_by(self, u: &Value) -> Tally {
        let mut t = self;
        for (i, k) in [
            "input_tokens",
            "output_tokens",
            "cache_read_input_tokens",
            "cache_creation_input_tokens",
        ]
        .iter()
        .enumerate()
        {
            if let Some(n) = num(u, k) {
                t.0[i] = n;
            }
        }
        t
    }
    fn plus(self, o: Tally) -> Tally {
        let mut t = self;
        for i in 0..4 {
            t.0[i] += o.0[i];
        }
        t
    }
}

impl ParseState {
    fn publish(&mut self, cost_usd: Option<f64>) {
        let t = self.closed.plus(self.open.unwrap_or_default());
        let r = TokenReport {
            input: Some(t.0[0]),
            output: Some(t.0[1]),
            cache_read: Some(t.0[2]),
            cache_create: Some(t.0[3]),
            // As reported: the daemon offers it only for an api account (decision 0043).
            cost_usd,
        };
        if self.last_live.as_ref() != Some(&r) {
            self.last_live = Some(r.clone());
            self.live = Some(r);
        }
    }

    /// The cumulative figures if they changed since the last call.
    pub fn take_live(&mut self) -> Option<TokenReport> {
        self.live.take()
    }

    /// Codex: a finished turn's figures are added to the invocation's.
    pub(crate) fn add_turn(&mut self, r: &TokenReport) {
        self.closed = self.closed.plus(Tally([
            r.input.unwrap_or(0),
            r.output.unwrap_or(0),
            r.cache_read.unwrap_or(0),
            r.cache_create.unwrap_or(0),
        ]));
        self.publish(None);
    }
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

/// The todo list of a `TodoWrite` call (`{"todos": [{content, status, activeForm}]}`).
/// None when the input is not that shape.
fn todo_items(input: &Value) -> Option<Vec<PlanItem>> {
    let list = input["todos"].as_array()?;
    Some(
        list.iter()
            .filter_map(|t| {
                let text = t["content"]
                    .as_str()
                    .or_else(|| t["activeForm"].as_str())?
                    .to_string();
                let status = match t["status"].as_str() {
                    Some("completed") => PlanStatus::Done,
                    Some("in_progress") => PlanStatus::Doing,
                    _ => PlanStatus::Pending,
                };
                Some(PlanItem { text, status })
            })
            .collect(),
    )
}

/// The skill a `Skill` call names (`{"skill": "mason:review"}`), on one line.
fn skill_name(input: &Value) -> Option<String> {
    let s = input["skill"].as_str()?.trim().trim_start_matches('/');
    let one: String = s.chars().filter(|c| !c.is_control()).collect();
    (!one.is_empty()).then(|| clip(&one, 100))
}

/// What a tool call is doing, in one line: "Editing src/lib.rs", "Running cargo test".
/// Not redacted here; the daemon redacts what it shows.
fn tool_summary(name: &str, input: &Value) -> String {
    let field = |k: &str| input[k].as_str().map(str::trim).filter(|s| !s.is_empty());
    let line = match name {
        "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => {
            field("file_path").map(|p| format!("Editing {p}"))
        }
        "Bash" => field("command").map(|c| format!("Running {c}")),
        "Read" => field("file_path").map(|p| format!("Reading {p}")),
        "Grep" | "Glob" => field("pattern").map(|p| format!("Searching for {p}")),
        "TodoWrite" => Some("Updating the todo list".to_string()),
        "Skill" => skill_name(input).map(|s| format!("Using skill {s}")),
        "mcp__mason__run_check" => field("name").map(|n| format!("Running check {n}")),
        _ => None,
    };
    // Anything else is named, never dumped: its arguments may hold anything.
    let line = line.unwrap_or_else(|| format!("Using {}", name.trim_start_matches("mcp__mason__")));
    clip(&line.split_whitespace().collect::<Vec<_>>().join(" "), 200)
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
            if let Some(w) = num(&v, "contextWindow").filter(|w| *w > 0) {
                st.context.reported = Some(w);
            }
            if let Some(m) = v["model"].as_str().filter(|m| !m.is_empty()) {
                st.context.model = m.to_string();
                out.push(AgentEvent::Model(m.to_string()));
            }
            if v["apiKeySource"].as_str().is_some_and(|k| k != "none") {
                st.billing = BillingKind::Api;
            }
        }
        (Some("system"), Some("api_retry")) => out.push(AgentEvent::ApiRetry {
            attempt: num(&v, "attempt").unwrap_or(0) as u32,
            status: num(&v, "error_status").map(|n| n as u32),
            delay_ms: num(&v, "retry_delay_ms"),
        }),
        (Some("system"), Some(subtype)) if subtype.contains("compact") => {
            let meta = [&v["compact_metadata"], &v["compactMetadata"]];
            let figure = |keys: [&str; 2]| {
                meta.iter()
                    .find_map(|m| keys.iter().find_map(|k| num(m, k)))
            };
            let before = figure(["pre_tokens", "preTokens"]).or(st.context.used);
            let after = figure(["post_tokens", "postTokens"]);
            // The next message's fill is a new one, however close to the last.
            st.context.used = None;
            st.context.sent = None;
            out.push(AgentEvent::Compacted {
                subtype: subtype.to_string(),
                before,
                after,
            });
        }
        (Some("assistant"), _) => {
            // A subagent's messages fill its own window, not this session's.
            if v["parent_tool_use_id"].is_null() {
                out.extend(st.context.message(&v["message"]["usage"]));
            }
            st.turns += 1;
            for block in v["message"]["content"].as_array().into_iter().flatten() {
                match block["type"].as_str() {
                    Some("text") => {
                        if let Some(t) = block["text"].as_str() {
                            out.push(AgentEvent::Text(t.to_string()));
                        }
                    }
                    Some("tool_use") => {
                        let name = block["name"].as_str().unwrap_or("?").to_string();
                        let input = &block["input"];
                        if name == "Skill" {
                            if let Some(s) = skill_name(input) {
                                out.push(AgentEvent::Skill { name: s });
                            }
                        }
                        if name == "TodoWrite" {
                            if let Some(items) = todo_items(input) {
                                out.push(AgentEvent::Plan { items });
                            }
                        }
                        out.push(AgentEvent::ToolCall {
                            summary: tool_summary(&name, input),
                            name: name.clone(),
                        });
                        out.extend(file_edits(&name, input, st.turns));
                    }
                    _ => {}
                }
            }
        }
        (Some("stream_event"), _) => {
            let e = &v["event"];
            match e["type"].as_str() {
                Some("message_start") => {
                    let open = st.open.take().unwrap_or_default();
                    st.closed = st.closed.plus(open);
                    st.open = Some(Tally::of(&e["message"]["usage"]));
                    st.publish(None);
                    if v["parent_tool_use_id"].is_null() {
                        out.extend(st.context.message(&e["message"]["usage"]));
                    }
                }
                Some("message_delta") => {
                    let cur = st.open.unwrap_or_default();
                    st.open = Some(cur.replaced_by(&e["usage"]));
                    st.publish(None);
                }
                _ => {}
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
            out.extend(st.context.result(&v["modelUsage"]));
            // The final figures replace the running tally.
            st.closed = Tally::of(u);
            st.open = None;
            st.publish(v["total_cost_usd"].as_f64());
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

/// The wording an agent CLI uses when it cannot authenticate, lowercased. The one
/// place these patterns live; each has a fixture in `fixtures/claude`.
pub const AUTH_FAILURE_WORDS: [&str; 5] = [
    "not logged in",
    "invalid api key",
    "please run /login",
    "authentication_error",
    "invalid x-api-key",
];

/// Does this say the CLI cannot authenticate? A 401 or 403 status, or the wording
/// in the result text (or on stderr when there is no result).
pub fn is_auth_failure(status: Option<u32>, text: &str) -> bool {
    if matches!(status, Some(401) | Some(403)) {
        return true;
    }
    let t = text.to_lowercase();
    AUTH_FAILURE_WORDS.iter().any(|w| t.contains(w))
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
        if is_auth_failure(None, stderr) {
            return Outcome::NotLoggedIn {
                detail: clip(stderr, 300),
            };
        }
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
    if is_auth_failure(r.api_error_status, &r.text) {
        return Outcome::NotLoggedIn {
            detail: clip(&r.text, 300),
        };
    }
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
    fn a_todo_write_call_gives_a_plan_and_tool_lines() {
        // Synthetic fixture (not a capture): see fixtures/claude/README.md.
        let (ev, _) = parse("synthetic_todo_write.jsonl");
        let items = ev
            .iter()
            .find_map(|e| match e {
                AgentEvent::Plan { items } => Some(items.clone()),
                _ => None,
            })
            .expect("a Plan event");
        let count = |s: PlanStatus| items.iter().filter(|i| i.status == s).count();
        assert_eq!(items.len(), 7);
        assert_eq!(
            (
                count(PlanStatus::Done),
                count(PlanStatus::Doing),
                count(PlanStatus::Pending)
            ),
            (3, 1, 3)
        );
        assert_eq!(
            items
                .iter()
                .find(|i| i.status == PlanStatus::Doing)
                .unwrap()
                .text,
            "Count tool calls"
        );
        let summaries: Vec<&str> = ev
            .iter()
            .filter_map(|e| match e {
                AgentEvent::ToolCall { summary, .. } => Some(summary.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            summaries,
            [
                "Updating the todo list",
                "Editing crates/daemon/src/views.rs",
                "Running cargo test"
            ]
        );
    }

    fn contexts(ev: &[AgentEvent]) -> Vec<(u64, u64)> {
        ev.iter()
            .filter_map(|e| match e {
                AgentEvent::Context { used, window } => Some((*used, *window)),
                _ => None,
            })
            .collect()
    }

    /// The peak percent and compaction count a step would record from these events,
    /// printed for the s1108 exit test.
    fn fill(name: &str, ev: &[AgentEvent]) -> (u64, u32) {
        let mut c = crate::runner::ContextStats::default();
        ev.iter().for_each(|e| c.observe(e));
        let peak = c.peak_used * 100 / c.window;
        println!("{name}: peak {peak}% compactions {}", c.compactions);
        (peak, c.compactions)
    }

    #[test]
    fn context_fill_of_the_captured_runs() {
        let (ev, _) = parse("real_tool_suspend.jsonl");
        // The second message moved less than a point, so it sends nothing.
        assert_eq!(contexts(&ev), [(34_300, 1_000_000)]);
        assert_eq!(fill("real_tool_suspend.jsonl", &ev), (3, 0));
        let (ev, _) = parse("real_plain.jsonl");
        assert_eq!(contexts(&ev), [(35_278, 1_000_000)]);
        let (ev, _) = parse("real_partial_messages.jsonl");
        assert_eq!(contexts(&ev), [(7_505, 1_000_000)]);
    }

    #[test]
    fn context_fill_around_a_compaction() {
        // Synthetic fixture (not a capture): see fixtures/claude/README.md.
        let (ev, _) = parse("synthetic_compaction.jsonl");
        assert_eq!(
            contexts(&ev),
            [
                (100_000, 200_000),
                (170_000, 200_000),
                (184_000, 200_000),
                (30_000, 200_000)
            ]
        );
        let compacted: Vec<&AgentEvent> = ev
            .iter()
            .filter(|e| matches!(e, AgentEvent::Compacted { .. }))
            .collect();
        assert_eq!(
            compacted,
            [&AgentEvent::Compacted {
                subtype: "compact_boundary".into(),
                before: Some(185_000),
                after: None,
            }]
        );
        assert_eq!(fill("synthetic_compaction.jsonl", &ev), (92, 1));
    }

    #[test]
    fn context_window_is_corrected_by_the_result_and_other_system_lines_pass() {
        let mut st = ParseState::default();
        let mut feed = |l: &str| parse_line(l, &mut st);
        feed(r#"{"type":"system","subtype":"init","model":"claude-new-9"}"#);
        let ev = feed(
            r#"{"type":"stream_event","event":{"type":"message_start","message":{"usage":{"input_tokens":50000}}}}"#,
        );
        assert_eq!(contexts(&ev), [(50_000, 200_000)]);
        assert!(feed(r#"{"type":"system","subtype":"thinking_tokens"}"#).is_empty());
        let ev = feed(
            r#"{"type":"result","is_error":false,"usage":{},"modelUsage":{"claude-new-9[1m]":{"contextWindow":1000000}}}"#,
        );
        assert_eq!(contexts(&ev), [(50_000, 1_000_000)]);
        // A later CLI's name for it is still seen, with the last fill as what was there.
        let ev = feed(r#"{"type":"system","subtype":"auto_compaction"}"#);
        assert_eq!(
            ev,
            [AgentEvent::Compacted {
                subtype: "auto_compaction".into(),
                before: Some(50_000),
                after: None,
            }]
        );
    }

    #[test]
    fn todowrite_is_offered_to_every_step() {
        let a = ClaudeAdapter::new("claude", "mason-mcp", std::env::temp_dir());
        for kind in [StepKind::Explore, StepKind::Develop, StepKind::Review] {
            let c = a.build_command(&spec(kind), None).unwrap();
            let i = c.args.iter().position(|x| x == "--tools").unwrap();
            assert!(c.args[i + 1].split(',').any(|t| t == "TodoWrite"));
            let j = c.args.iter().position(|x| x == "--allowedTools").unwrap();
            assert!(c.args[j + 1..].iter().any(|t| t == "TodoWrite"));
        }
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
    fn partial_messages_give_live_figures_that_end_at_the_result() {
        let mut st = ParseState::default();
        let mut lives = vec![];
        let mut result = None;
        for l in fixture("real_partial_messages.jsonl").lines() {
            for e in parse_line(l, &mut st) {
                if let AgentEvent::Tokens(t) = e {
                    result = Some(t);
                }
            }
            if let Some(t) = st.take_live() {
                lives.push(t);
            }
        }
        let result = result.unwrap();
        let last = lives.last().unwrap();
        assert_eq!(
            (last.input, last.output, last.cache_read, last.cache_create),
            (
                result.input,
                result.output,
                result.cache_read,
                result.cache_create
            )
        );
        assert_eq!(result.output, Some(988));
        // After message 1 is over (its delta) the figure is that message's, not 16.
        assert!(lives.iter().any(|t| t.output == Some(137)), "{lives:?}");
        // After message 2's delta: 137 + 851, exactly the result's output.
        assert!(lives.iter().any(|t| t.output == Some(988)));
        // Never goes down.
        assert!(lives.windows(2).all(|w| w[1].output >= w[0].output));
    }

    #[test]
    fn a_repeated_assistant_event_does_not_change_the_tally() {
        let mut st = ParseState::default();
        let lines: Vec<String> = fixture("real_partial_messages.jsonl")
            .lines()
            .map(String::from)
            .collect();
        let before_assistant = lines
            .iter()
            .position(|l| l.contains("\"type\":\"assistant\""))
            .unwrap();
        for l in &lines[..before_assistant] {
            parse_line(l, &mut st);
        }
        st.take_live();
        for l in lines
            .iter()
            .filter(|l| l.contains("\"type\":\"assistant\""))
        {
            parse_line(l, &mut st);
            parse_line(l, &mut st);
        }
        assert_eq!(st.take_live(), None, "assistant events carry no figure");
    }

    #[test]
    fn a_plain_stream_has_a_live_figure_only_at_the_result() {
        let mut st = ParseState::default();
        let mut n = 0;
        for l in fixture("real_plain.jsonl").lines() {
            parse_line(l, &mut st);
            if st.take_live().is_some() {
                n += 1;
            }
        }
        assert_eq!(n, 1);
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
        assert_eq!(w.state, WindowState::Ok, "status allowed");
        assert_eq!(w.id, "five_hour", "the window named in the event");
        assert!(w.resets_at.is_some());
        assert!(
            u.windows.iter().any(|x| x.id == "seven_day"),
            "all windows kept"
        );
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
    fn a_logged_out_cli_is_a_credentials_hold_not_a_crash() {
        // Was pinned as a crash (task #95): a crash fails the task, but an expired
        // login fails every task the same way and only the owner can fix it, so
        // it is a hold with the `credentials` alert.
        for f in [
            "synthetic_not_logged_in.jsonl",
            "synthetic_auth_401.jsonl",
            "synthetic_auth_403.jsonl",
            "synthetic_invalid_api_key.jsonl",
            "synthetic_authentication_error.jsonl",
        ] {
            let (_, st) = parse(f);
            match final_outcome(&st, false, Some(1), "") {
                Outcome::NotLoggedIn { detail } => assert!(!detail.is_empty(), "{f}"),
                o => panic!("{f}: {o:?}"),
            }
        }
    }

    #[test]
    fn the_same_wording_on_stderr_with_no_result_is_not_logged_in() {
        let st = ParseState::default();
        for e in [
            "Error: Not logged in \u{b7} Please run /login",
            "Invalid API key",
            "authentication_error: bad token",
        ] {
            assert!(
                matches!(
                    final_outcome(&st, false, Some(1), e),
                    Outcome::NotLoggedIn { .. }
                ),
                "{e}"
            );
        }
    }

    #[test]
    fn a_plain_error_result_stays_a_crash() {
        let mut st = ParseState::default();
        parse_line(
            r#"{"type":"result","is_error":true,"api_error_status":400,"result":"bad request"}"#,
            &mut st,
        );
        assert!(matches!(
            final_outcome(&st, false, Some(1), ""),
            Outcome::Crash { .. }
        ));
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
        assert_eq!(get("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT"), Some("600000"));
        assert_eq!(get("EXTRA"), Some("1"));
        assert_eq!(get("MASON_STEP_KIND"), Some("develop"));
        assert_eq!(env.iter().filter(|(k, _)| k == "GIT_ASKPASS").count(), 1);
    }

    #[test]
    fn the_env_file_is_parsed_without_a_shell() {
        let text = "# comment\n#export CLAUDE_CODE_OAUTH_TOKEN=old\n\n\
                    export CLAUDE_CODE_OAUTH_TOKEN=first\n\
                    CLAUDE_CODE_OAUTH_TOKEN='tok' # note\n\
                    ANTHROPIC_BASE_URL=\"http://127.0.0.1:1\"\n\
                    ANTHROPIC_API_KEY=\nPATH=/evil\nnot a line\n";
        let want = [
            ("CLAUDE_CODE_OAUTH_TOKEN", "tok"),
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:1"),
        ];
        assert_eq!(
            parse_env_file(text),
            want.map(|(k, v)| (k.to_string(), v.to_string()))
        );
    }

    #[test]
    fn the_env_file_is_read_for_every_session() {
        let mut a = adapter(&[
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_API_KEY", "inherited"),
            ("CLAUDE_CODE_OAUTH_TOKEN", "inherited"),
        ]);
        let file = a
            .state_dir
            .with_file_name(format!("mason-claude-env-{}", std::process::id()));
        let _ = std::fs::remove_file(&file);
        a.env_file = Some(file.clone());
        let creds = |resume| {
            let c = a.build_command(&spec(StepKind::Develop), resume).unwrap();
            let get = |k: &str| c.env.iter().find(|(ek, _)| ek == k).map(|(_, v)| v.clone());
            (get("CLAUDE_CODE_OAUTH_TOKEN"), get("ANTHROPIC_API_KEY"))
        };
        let inherited = Some("inherited".to_string());
        assert_eq!(creds(None), (inherited.clone(), inherited), "no file");
        for (token, resume) in [("one", None), ("two", Some(("s1", "go on")))] {
            std::fs::write(&file, format!("export CLAUDE_CODE_OAUTH_TOKEN={token}\n")).unwrap();
            assert_eq!(creds(resume), (Some(token.to_string()), None));
        }
        std::fs::remove_file(&file).unwrap();
    }

    #[test]
    fn read_only_steps_get_read_only_tools_and_no_remote_git() {
        let a = adapter(&[
            ("PATH", "/usr/bin"),
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:1"),
        ]);
        let c = a.build_command(&spec(StepKind::Review), None).unwrap();
        let tools_at = c.args.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(c.args[tools_at + 1], "Read,Grep,Glob,TodoWrite,Skill");
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
    fn every_session_is_given_the_command_guard() {
        let a = adapter(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")]);
        let mut sp = spec(StepKind::Develop);
        sp.env = vec![("MASON_GUARD_WORKTREE".into(), "/".into())];
        for resume in [None, Some(("sess-1", "note"))] {
            let c = a.build_command(&sp, resume).unwrap();
            let after = |flag: &str| {
                let at = c.args.iter().position(|x| x == flag).unwrap();
                PathBuf::from(&c.args[at + 1])
            };
            let file = after("--settings");
            assert_eq!(file.parent(), after("--mcp-config").parent());
            let v: Value = serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap();
            let hook = &v["hooks"]["PreToolUse"][0];
            assert_eq!(hook["matcher"], "Bash");
            let command = hook["hooks"][0]["command"].as_str().unwrap();
            // Joined as `write_settings` joins it: the separator differs on Windows.
            let mason = a
                .mcp_bin
                .with_file_name(format!("mason{}", std::env::consts::EXE_SUFFIX));
            let guard = format!("'{}' hook guard ||", mason.display());
            assert!(command.starts_with(&guard), "{command}");
            assert!(command.contains("exit 2"), "{command}");
            let edit = &v["hooks"]["PreToolUse"][2];
            assert_eq!(edit["matcher"], "Edit|Write|MultiEdit");
            let command = edit["hooks"][0]["command"].as_str().unwrap();
            assert_eq!(command, format!("'{}' hook edit", mason.display()));
            let env = |k: &str| {
                let all: Vec<&str> = c
                    .env
                    .iter()
                    .filter(|(ek, _)| ek == k)
                    .map(|(_, v)| v.as_str())
                    .collect();
                all
            };
            assert_eq!(env("MASON_GUARD_WORKTREE"), ["/work"]);
            assert_eq!(
                env("MASON_GUARD_STATE"),
                [a.state_dir.display().to_string()]
            );
        }
    }

    #[test]
    fn every_step_kind_is_given_the_skill_tool() {
        let a = adapter(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")]);
        for kind in [
            StepKind::Explore,
            StepKind::Develop,
            StepKind::Review,
            StepKind::Diagnose,
            StepKind::Chat,
            StepKind::Health,
        ] {
            let c = a.build_command(&spec(kind), None).unwrap();
            let at = |flag: &str| c.args.iter().position(|x| x == flag).unwrap();
            assert!(c.args[at("--tools") + 1].split(',').any(|t| t == "Skill"));
            let allowed = &c.args[at("--allowedTools") + 1..at("--disallowedTools")];
            assert!(allowed.iter().any(|t| t == "Skill"), "{kind:?}");
        }
    }

    fn settings_of(c: &AgentCommand) -> Value {
        let at = c.args.iter().position(|x| x == "--settings").unwrap();
        serde_json::from_str(&std::fs::read_to_string(&c.args[at + 1]).unwrap()).unwrap()
    }

    #[test]
    fn project_skills_are_passed_as_a_plugin_of_the_sessions_own() {
        let mut a = adapter(&[("ANTHROPIC_BASE_URL", "http://127.0.0.1:1")]);
        let c = a.build_command(&spec(StepKind::Review), None).unwrap();
        assert!(
            !c.args.iter().any(|x| x == "--plugin-dir"),
            "no skills, no directory"
        );
        a.skills.project = vec![crate::skills::SkillFile {
            path: "review/SKILL.md".into(),
            data: b"---\nname: review\n---\nbody\n".to_vec(),
            executable: false,
        }];
        let dir_of = |c: &AgentCommand| {
            let at = c.args.iter().position(|x| x == "--plugin-dir").unwrap();
            PathBuf::from(&c.args[at + 1])
        };
        let first = a.build_command(&spec(StepKind::Review), None).unwrap();
        let again = a
            .build_command(&spec(StepKind::Review), Some(("sess-1", "note")))
            .unwrap();
        assert_ne!(dir_of(&first), dir_of(&again), "per session");
        for c in [&first, &again] {
            let dir = dir_of(c);
            assert!(dir.starts_with(&a.state_dir));
            assert!(dir.join("skills/review/SKILL.md").is_file());
            let hook = &settings_of(c)["hooks"]["PreToolUse"][1];
            assert_eq!(hook["matcher"], "Skill");
            let command = hook["hooks"][0]["command"].as_str().unwrap();
            assert!(command.contains(" hook skill --policy '"), "{command}");
            assert!(command.contains("exit 2"), "{command}");
            let policy = dir.with_file_name("skills.json");
            assert!(command.contains(&policy.display().to_string()));
            assert!(std::fs::read_to_string(policy)
                .unwrap()
                .contains("mason:review"));
        }
    }

    #[test]
    fn user_skills_off_is_written_into_the_sessions_settings() {
        let home = tempfile::tempdir().unwrap();
        let md = home.path().join(".claude/skills/owners/SKILL.md");
        std::fs::create_dir_all(md.parent().unwrap()).unwrap();
        std::fs::write(md, "x").unwrap();
        let mut a = adapter(&[
            ("ANTHROPIC_BASE_URL", "http://127.0.0.1:1"),
            ("HOME", &home.path().display().to_string()),
        ]);
        let c = a.build_command(&spec(StepKind::Develop), None).unwrap();
        assert!(settings_of(&c)["skillOverrides"]["owners"].is_null());
        a.skills.user = false;
        let c = a.build_command(&spec(StepKind::Develop), None).unwrap();
        assert_eq!(settings_of(&c)["skillOverrides"]["owners"], "off");
        assert!(
            !c.args.iter().any(|x| x == "--setting-sources"),
            "the rest of the owner's settings still apply"
        );
    }

    #[test]
    fn edit_write_and_multiedit_calls_make_file_edit_events_capped() {
        let mut st = ParseState::default();
        let big = "é".repeat(mason_adapter_api::FILE_EDIT_TEXT_MAX);
        let line = |blocks: Value| {
            serde_json::json!({"type":"assistant","message":{"content":blocks}}).to_string()
        };
        let edits = |ev: Vec<AgentEvent>| -> Vec<AgentEvent> {
            ev.into_iter()
                .filter(|e| matches!(e, AgentEvent::FileEdit { .. }))
                .collect()
        };
        let ev = edits(parse_line(
            &line(serde_json::json!([
                {"type":"tool_use","id":"1","name":"Edit","input":{"file_path":"a.rs","old_string":"x","new_string":"y"}},
                {"type":"tool_use","id":"2","name":"Write","input":{"file_path":"b.rs","content":"hi"}},
                {"type":"tool_use","id":"3","name":"Read","input":{"file_path":"c.rs"}}
            ])),
            &mut st,
        ));
        let fe =
            |tool: &str, path: &str, old: Option<&str>, new: &str, turn| AgentEvent::FileEdit {
                tool: tool.into(),
                path: path.into(),
                old: old.map(str::to_string),
                new: new.into(),
                truncated: false,
                turn,
            };
        assert_eq!(
            ev,
            [
                fe("Edit", "a.rs", Some("x"), "y", 1),
                fe("Write", "b.rs", None, "hi", 1)
            ]
        );
        let ev = edits(parse_line(
            &line(serde_json::json!([
                {"type":"tool_use","id":"4","name":"MultiEdit","input":{"file_path":"d.rs","edits":[
                    {"old_string":"a","new_string":"b"},{"old_string":"b","new_string":big}]}}
            ])),
            &mut st,
        ));
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[0], fe("MultiEdit", "d.rs", Some("a"), "b", 2));
        let AgentEvent::FileEdit { new, truncated, .. } = &ev[1] else {
            panic!()
        };
        assert!(*truncated);
        assert_eq!(new.len(), mason_adapter_api::FILE_EDIT_TEXT_MAX);
    }

    #[test]
    fn a_skill_call_is_reported_by_name() {
        let mut st = ParseState::default();
        let ev = parse_line(
            r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Skill","input":{"skill":"mason:review"}}]}}"#,
            &mut st,
        );
        assert_eq!(
            ev,
            [
                AgentEvent::Skill {
                    name: "mason:review".into()
                },
                AgentEvent::ToolCall {
                    name: "Skill".into(),
                    summary: "Using skill mason:review".into()
                }
            ]
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
