// SPDX-License-Identifier: GPL-3.0-or-later

//! Running a plugin process and talking to it. Every call has a timeout; a plugin
//! that does not answer in time is killed. Its stderr is copied to a log file.

use crate::rpc::{self, Initialized, RpcError, StepResult};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// How to start a plugin.
#[derive(Clone, Debug)]
pub struct Launch {
    /// The extension's folder: the working directory.
    pub dir: PathBuf,
    /// Program first.
    pub run: Vec<String>,
    /// Where stderr goes (appended). `None`: dropped.
    pub log: Option<PathBuf>,
}

#[derive(Debug)]
pub enum CallError {
    /// No answer within the time allowed. The process has been killed.
    Timeout(Duration),
    /// The process ended or wrote something that is not protocol.
    Closed(String),
    /// The plugin answered with an error.
    Rpc(RpcError),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallError::Timeout(d) => write!(f, "no answer within {}s", d.as_secs_f32().ceil()),
            CallError::Closed(s) => write!(f, "{s}"),
            CallError::Rpc(e) => write!(f, "the plugin refused: {} (code {})", e.message, e.code),
        }
    }
}

pub struct Plugin {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Result<String, String>>,
    next_id: u64,
}

impl Plugin {
    pub fn spawn(l: &Launch) -> std::io::Result<Plugin> {
        let program = l
            .run
            .first()
            .ok_or_else(|| std::io::Error::other("the manifest's `run` is empty"))?;
        let stderr = match &l.log {
            Some(p) => {
                if let Some(parent) = p.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                Stdio::from(
                    std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(p)?,
                )
            }
            None => Stdio::null(),
        };
        let mut child = Command::new(program)
            .args(&l.run[1..])
            .current_dir(&l.dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()?;
        let stdout = child.stdout.take().expect("piped");
        let stdin = child.stdin.take();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let item = line.map_err(|e| e.to_string());
                let stop = item.is_err();
                if tx.send(item).is_err() || stop {
                    return;
                }
            }
            let _ = tx.send(Err("the plugin closed its output".into()));
        });
        Ok(Plugin {
            child,
            stdin,
            lines,
            next_id: 1,
        })
    }

    /// One request and its answer. Lines that are not this request's answer
    /// (notifications) are skipped.
    pub fn call(
        &mut self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, CallError> {
        self.call_noting(method, params, timeout, &mut |_| {})
    }

    /// [`Plugin::call`], handing the params of every `step.progress` notification to
    /// `note` as it arrives. Notifications do not extend the time allowed.
    pub fn call_noting(
        &mut self,
        method: &str,
        params: &Value,
        timeout: Duration,
        note: &mut dyn FnMut(&Value),
    ) -> Result<Value, CallError> {
        let id = self.next_id;
        self.next_id += 1;
        let line = rpc::request(id, method, params);
        let sent = self
            .stdin
            .as_mut()
            .map(|s| writeln!(s, "{line}").and_then(|()| s.flush()));
        if !matches!(sent, Some(Ok(()))) {
            self.kill();
            return Err(CallError::Closed(format!(
                "cannot send `{method}`: the plugin is not reading its input{}",
                self.exit_note()
            )));
        }
        let end = Instant::now() + timeout;
        loop {
            let left = end.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(Ok(l)) if l.trim().is_empty() => {}
                Ok(Ok(l)) if rpc::read_progress_line(&l).is_some() => {
                    if let Some(p) = rpc::read_progress_line(&l) {
                        note(&p);
                    }
                }
                Ok(Ok(l)) => match rpc::parse_response(&l) {
                    Ok(Some(r)) if r.id == Some(id) => return r.result.map_err(CallError::Rpc),
                    Ok(_) => {}
                    Err(e) => {
                        self.kill();
                        return Err(CallError::Closed(format!("`{method}`: {e}")));
                    }
                },
                Ok(Err(e)) => {
                    self.kill();
                    return Err(CallError::Closed(format!(
                        "`{method}`: {e}{}",
                        self.exit_note()
                    )));
                }
                Err(RecvTimeoutError::Timeout) => {
                    self.kill();
                    return Err(CallError::Timeout(timeout));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.kill();
                    return Err(CallError::Closed(format!(
                        "`{method}`: the plugin went away"
                    )));
                }
            }
        }
    }

    fn exit_note(&mut self) -> String {
        match self.child.try_wait() {
            Ok(Some(s)) => format!(" ({s})"),
            _ => String::new(),
        }
    }

    /// Ask it to shut down, and wait for the process to exit.
    pub fn shutdown(&mut self, timeout: Duration) -> Result<(), String> {
        self.call("shutdown", &json!({}), timeout)
            .map_err(|e| format!("shutdown: {e}"))?;
        self.stdin = None; // closing its input is the signal to stop reading
        let end = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return Ok(()),
                Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
                _ => {
                    self.kill();
                    return Err(format!(
                        "the plugin did not exit within {}s of shutdown",
                        timeout.as_secs_f32().ceil()
                    ));
                }
            }
        }
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Plugin {
    fn drop(&mut self) {
        self.kill();
    }
}

/// `initialize`, checked for shape.
pub fn initialize(p: &mut Plugin, timeout: Duration) -> Result<Initialized, String> {
    initialize_as(p, timeout, crate::PROTOCOL)
}

/// `initialize` for the protocol version the manifest names.
pub fn initialize_as(
    p: &mut Plugin,
    timeout: Duration,
    protocol: u32,
) -> Result<Initialized, String> {
    let v = p
        .call(
            "initialize",
            &json!({"protocol": protocol, "host": "mason"}),
            timeout,
        )
        .map_err(|e| format!("initialize: {e}"))?;
    rpc::read_initialized(&v)
}

/// What a step run needs to say about the task.
#[derive(Clone, Debug)]
pub struct StepRequest {
    pub step: String,
    pub step_type: String,
    pub config: Value,
    pub task: u64,
    pub title: String,
    pub worktree: String,
    pub env: Value,
}

pub fn step_run(
    p: &mut Plugin,
    r: &StepRequest,
    timeout: Duration,
) -> Result<StepResult, CallError> {
    step_run_noting(p, r, timeout, &mut |_| {})
}

/// [`step_run`], handing each `step.progress` notification's params to `note`.
pub fn step_run_noting(
    p: &mut Plugin,
    r: &StepRequest,
    timeout: Duration,
    note: &mut dyn FnMut(&Value),
) -> Result<StepResult, CallError> {
    let v = p.call_noting(
        "step.run",
        &json!({
            "step": r.step, "type": r.step_type, "config": r.config, "task": r.task,
            "title": r.title, "worktree": r.worktree, "env": r.env,
        }),
        timeout,
        note,
    )?;
    rpc::read_step_result(&v).map_err(CallError::Closed)
}

/// Start the plugin, check what it says it is, run the step, shut it down.
pub fn run_step(
    l: &Launch,
    expect_name: &str,
    r: &StepRequest,
    protocol: u32,
    init_timeout: Duration,
    run_timeout: Duration,
    note: &mut dyn FnMut(&Value),
) -> Result<StepResult, String> {
    let mut p = Plugin::spawn(l).map_err(|e| format!("cannot start the plugin: {e}"))?;
    let init = initialize_as(&mut p, init_timeout, protocol)?;
    if init.name != expect_name {
        return Err(format!(
            "the plugin says it is `{}`, but its manifest says `{expect_name}`",
            init.name
        ));
    }
    if init.protocol != protocol {
        return Err(format!(
            "the plugin speaks protocol {}, not {protocol}",
            init.protocol
        ));
    }
    // Only a protocol 2 plugin may send progress; a protocol 1 plugin's is ignored.
    let res = step_run_noting(&mut p, r, run_timeout, &mut |v| {
        if protocol >= 2 {
            note(v)
        }
    })
    .map_err(|e| format!("step.run: {e}"))?;
    // A plugin that will not shut down cleanly is stopped; the result stands.
    if p.shutdown(init_timeout).is_err() {
        p.kill();
    }
    Ok(res)
}
