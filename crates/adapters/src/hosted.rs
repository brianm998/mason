// SPDX-License-Identifier: GPL-3.0-or-later

//! Running an agent CLI under a step host, so the session outlives the
//! controller. The session is followed through the host's journal; after a
//! restart [`attach`] picks the same session up again from byte zero.

use crate::claude::{decide, parse_line, Dialect, ParseState};
use mason_adapter_api::{AdapterError, AgentEvent, Canceller, Outcome, SessionHandle, SessionSpec};
use mason_step_host::client::{ClientError, Event, HostClient};
use mason_step_host::launcher::{spawn_host, SpawnOptions};
use mason_step_host::protocol::{CURRENT, PREVIOUS};
use mason_step_host::registry::{read_journal, status_of, ExitRecord, HostState};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct HostLaunch {
    pub host_bin: PathBuf,
    /// The registry root. Keep it short: the host's socket lives under it.
    pub root: PathBuf,
    pub heartbeat: Duration,
    /// How long a finished host waits for its result to be collected.
    pub linger: Duration,
}

/// Invocation ids for `key`, sorted: `key-1`, `key-2`, …
fn invocations(root: &Path, key: &str) -> Vec<(u32, String)> {
    let prefix = format!("{key}-");
    let mut v: Vec<(u32, String)> = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            let n = name.strip_prefix(&prefix)?.parse::<u32>().ok()?;
            e.path().is_dir().then_some((n, name))
        })
        .collect();
    v.sort();
    v
}

pub fn latest_invocation(root: &Path, key: &str) -> Option<String> {
    invocations(root, key).pop().map(|(_, id)| id)
}

/// Start a host for the next invocation of `key`. Returns its id.
pub fn spawn(
    h: &HostLaunch,
    key: &str,
    program: &Path,
    args: &[String],
    env: &[(String, String)],
    cwd: &Path,
    stdin: &str,
) -> Result<String, AdapterError> {
    let n = invocations(&h.root, key).last().map_or(1, |(n, _)| n + 1);
    let id = format!("{key}-{n}");
    std::fs::create_dir_all(&h.root).map_err(AdapterError::Spawn)?;
    let input = h.root.join(format!("{id}.in"));
    std::fs::write(&input, stdin).map_err(AdapterError::Spawn)?;
    let mut cmd = vec![program.display().to_string()];
    cmd.extend(args.iter().cloned());
    let opts = SpawnOptions {
        host_bin: h.host_bin.clone(),
        root: h.root.clone(),
        id: id.clone(),
        cmd,
        cwd: Some(cwd.to_path_buf()),
        env: env.to_vec(),
        clear_env: true,
        stdin: Some(input),
        linger: h.linger,
        heartbeat: h.heartbeat,
    };
    spawn_host(&opts).map_err(AdapterError::Spawn)?;
    Ok(id)
}

fn stop_host(sock: &Path) {
    if let Ok(mut c) = HostClient::connect(sock, PREVIOUS, CURRENT, "adapter") {
        let _ = c.stop(500);
    }
}

struct Feed {
    st: ParseState,
    partial: Vec<u8>,
    stderr: String,
    tx: Sender<AgentEvent>,
    session_id: Arc<Mutex<Option<String>>>,
    progress: Option<mason_adapter_api::ProgressSink>,
    on_tokens: Option<mason_adapter_api::TokensSink>,
    stream: String,
}

impl Feed {
    /// Output at an absolute journal offset: counted, then parsed.
    fn chunk(&mut self, offset: u64, b: &[u8]) {
        if let Some(p) = &self.progress {
            p(&self.stream, offset, b);
        }
        self.bytes(b);
    }

    fn bytes(&mut self, b: &[u8]) {
        self.partial.extend_from_slice(b);
        while let Some(i) = self.partial.iter().position(|c| *c == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=i).collect();
            self.line(&String::from_utf8_lossy(&line[..line.len() - 1]));
        }
    }

    fn flush(&mut self) {
        if !self.partial.is_empty() {
            let rest = std::mem::take(&mut self.partial);
            self.line(&String::from_utf8_lossy(&rest));
        }
    }

    fn line(&mut self, l: &str) {
        let evs = parse_line(l, &mut self.st);
        if evs.is_empty()
            && serde_json::from_str::<serde_json::Value>(l).is_err()
            && !l.trim().is_empty()
        {
            self.stderr.push_str(l);
            self.stderr.push('\n');
            if self.stderr.len() > 4000 {
                let cut = self.stderr.len() - 2000;
                self.stderr = self.stderr[cut..].to_string();
            }
        }
        if let Some(id) = &self.st.session_id {
            *self.session_id.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.clone());
        }
        if let (Some(f), Some(t)) = (&self.on_tokens, self.st.take_live()) {
            f(&self.stream, &t);
        }
        for e in evs {
            let _ = self.tx.send(e);
        }
    }
}

/// Follow a host's journal to the end and report how the session ended.
pub fn attach(h: &HostLaunch, id: String, spec: &SessionSpec) -> Box<dyn SessionHandle> {
    attach_as(h, id, spec, Dialect::Claude)
}

/// [`attach`] for a CLI whose output is `dialect`.
pub fn attach_as(
    h: &HostLaunch,
    id: String,
    spec: &SessionSpec,
    dialect: Dialect,
) -> Box<dyn SessionHandle> {
    let dir = h.root.join(&id);
    let sock = dir.join("sock");
    let (tx, rx) = channel();
    let session_id = Arc::new(Mutex::new(None));
    let cancelled = Arc::new(AtomicBool::new(false));
    let timed_out = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));

    if let Some(d) = spec.deadline {
        let (sock, timed_out, done) = (sock.clone(), timed_out.clone(), done.clone());
        thread::spawn(move || {
            let end = Instant::now() + d;
            while Instant::now() < end {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            timed_out.store(true, Ordering::SeqCst);
            stop_host(&sock);
        });
    }

    let accepted = spec.accepted.clone();
    {
        let progress = spec.progress.clone();
        let on_tokens = spec.on_tokens.clone();
        let (sid, cancelled, timed_out, done) = (
            session_id.clone(),
            cancelled.clone(),
            timed_out.clone(),
            done.clone(),
        );
        thread::spawn(move || {
            let mut feed = Feed {
                st: ParseState::new(dialect),
                partial: vec![],
                stderr: String::new(),
                tx: tx.clone(),
                session_id: sid,
                progress,
                on_tokens,
                stream: dir
                    .file_name()
                    .map_or(String::new(), |n| n.to_string_lossy().into_owned()),
            };
            let exit = follow(&dir, &sock, &mut feed);
            feed.flush();
            done.store(true, Ordering::SeqCst);
            let outcome = if timed_out.load(Ordering::SeqCst) {
                Outcome::Timeout
            } else if cancelled.load(Ordering::SeqCst) {
                Outcome::NoResult
            } else {
                match exit {
                    Some(rec) => decide(
                        &feed.st,
                        accepted.is_set(),
                        (rec.signal == 0).then_some(rec.code),
                        &feed.stderr,
                    ),
                    None => Outcome::Crash {
                        detail: "the step host was lost before the agent finished".into(),
                    },
                }
            };
            let _ = tx.send(AgentEvent::Finished(outcome));
        });
    }

    let sock2 = h.root.join(&id).join("sock");
    let cancel_flag = cancelled.clone();
    let canceller: Canceller = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        stop_host(&sock2);
    });
    Box::new(Session {
        rx,
        session_id,
        canceller,
        finished: false,
    })
}

/// Stream the journal into `feed`. `Some(record)` if the child exited,
/// `None` if the host died without one.
fn follow(dir: &Path, sock: &Path, feed: &mut Feed) -> Option<ExitRecord> {
    let mut have = 0u64;
    loop {
        if let Ok(mut c) = HostClient::connect(sock, PREVIOUS, CURRENT, "adapter") {
            if c.attach(have).is_ok() {
                loop {
                    match c.next_event() {
                        Ok(Event::Data { offset, bytes }) if offset == have => {
                            let at = have;
                            have += bytes.len() as u64;
                            feed.chunk(at, &bytes);
                        }
                        Ok(Event::Exit(mut rec)) => {
                            rec.ended_ms = ExitRecord::read(dir).map_or(0, |r| r.ended_ms);
                            let _ = c.release();
                            return Some(rec);
                        }
                        Ok(Event::Heartbeat { .. }) => {}
                        Ok(Event::Data { .. }) | Err(_) => break,
                    }
                }
            }
        }
        // The socket is unavailable: the host may have finished and left, or died.
        if let Ok(rec) = ExitRecord::read(dir) {
            drain_disk(dir, &mut have, rec.output_len, feed);
            return Some(rec);
        }
        match status_of(dir) {
            Ok(s) if s.state == HostState::Lost => {
                drain_disk(dir, &mut have, u64::MAX, feed);
                return None;
            }
            Err(_) => return None,
            _ => thread::sleep(Duration::from_millis(50)),
        }
    }
}

fn drain_disk(dir: &Path, have: &mut u64, until: u64, feed: &mut Feed) {
    while *have < until {
        let want = ((until - *have).min(64 * 1024)) as usize;
        match read_journal(dir, *have, want) {
            Ok(b) if !b.is_empty() => {
                let at = *have;
                *have += b.len() as u64;
                feed.chunk(at, &b);
            }
            _ => break,
        }
    }
}

struct Session {
    rx: Receiver<AgentEvent>,
    session_id: Arc<Mutex<Option<String>>>,
    canceller: Canceller,
    finished: bool,
}

impl SessionHandle for Session {
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
        self.canceller.clone()
    }
}

#[allow(dead_code)]
fn _assert_client_error_is_used(_: ClientError) {}
