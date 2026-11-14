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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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

/// The number the next invocation of `key` takes. A host that never came up
/// leaves no directory but its input file, and that still counts: its number is
/// not used again, so every attempt stays on record.
fn next_invocation(root: &Path, key: &str) -> u32 {
    let from_dirs = invocations(root, key).last().map_or(0, |(n, _)| *n);
    let prefix = format!("{key}-");
    let from_inputs = std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_prefix(&prefix)?
                .strip_suffix(".in")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .unwrap_or(0);
    from_dirs.max(from_inputs) + 1
}

/// How the invocations of one step run, under every key it used (`key`, and
/// `keyu1`, … after the retry ladder), ended: the ids of those whose host was
/// lost or never came up, oldest first. A directory is a record that survives a
/// daemon restart, so this is how losses are counted across one.
pub fn lost_invocations(root: &Path, key: &str) -> Vec<String> {
    let mut out: Vec<(u32, u32, String)> = vec![];
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(id) = name.strip_suffix(".in") else {
            continue;
        };
        let Some(rest) = id.strip_prefix(key) else {
            continue;
        };
        // `-N`, or `uK-N` for a ladder retry: nothing else belongs to this run.
        let (k, tail) = match rest.strip_prefix('u') {
            Some(r) => {
                let digits = r.len() - r.trim_start_matches(|c: char| c.is_ascii_digit()).len();
                match r[..digits].parse::<u32>() {
                    Ok(k) => (k, &r[digits..]),
                    Err(_) => continue,
                }
            }
            None => (0, rest),
        };
        let Some(n) = tail.strip_prefix('-').and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let gone = silent_stop(&root.join(id)).is_some()
            || match status_of(&root.join(id)) {
                Ok(s) => s.state == HostState::Lost,
                Err(_) => true,
            };
        if gone {
            out.push((k, n, id.to_string()));
        }
    }
    out.sort();
    out.into_iter().map(|(_, _, id)| id).collect()
}

/// How close two hosts' last heartbeats must be for their loss to be one event.
pub const MASS_LOSS_WINDOW_MS: u64 = 30_000;

/// The step run an invocation id belongs to: `t7r2` of `t7r2-3` or `t7r2u1-3`.
fn run_of(id: &str) -> &str {
    let end = id.find(['-', 'u']).unwrap_or(id.len());
    &id[..end]
}

fn last_beat(root: &Path, id: &str) -> Option<u64> {
    std::fs::read_to_string(root.join(id).join("hb"))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Was the host `id` lost together with the host of another step run: their last
/// heartbeats within [`MASS_LOSS_WINDOW_MS`] of each other? Then the machine, a
/// daemon restart or a mass kill took them, not anything about this run, and the
/// loss does not count against its restarts.
pub fn lost_with_others(root: &Path, id: &str) -> bool {
    let Some(mine) = last_beat(root, id) else {
        return false;
    };
    let me = run_of(id);
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(other) = name.strip_suffix(".in") else {
            continue;
        };
        if run_of(other) == me || !other.starts_with('t') {
            continue;
        }
        let gone = matches!(status_of(&root.join(other)), Ok(s) if s.state == HostState::Lost);
        if !gone {
            continue;
        }
        if last_beat(root, other).is_some_and(|b| b.abs_diff(mine) <= MASS_LOSS_WINDOW_MS) {
            return true;
        }
    }
    false
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
    let n = next_invocation(&h.root, key);
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
    spawn_host(&opts).map_err(AdapterError::HostStart)?;
    Ok(id)
}

/// The file MASON leaves in a host's directory when it stopped the session for
/// going silent. The stop leaves a normal exit record, so this is what makes the
/// invocation count as one that has to be continued.
const SILENT_MARK: &str = "silent";

/// Why this host's session was stopped for silence, if it was.
pub fn silent_stop(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(SILENT_MARK)).ok()
}

/// Ask the host to stop. Returns whether the request was delivered.
fn stop_host(sock: &Path) -> bool {
    match HostClient::connect(sock, PREVIOUS, CURRENT, "adapter") {
        Ok(mut c) => c.stop(500).is_ok(),
        Err(_) => false,
    }
}

/// Keep asking the host to stop until the session has ended (or a minute has passed). One
/// attempt could be lost to a host slow to accept on a loaded machine, and the session would
/// then run on past its deadline.
fn stop_host_until_done(sock: &Path, done: &AtomicBool) {
    let end = Instant::now() + Duration::from_secs(60);
    while !done.load(Ordering::SeqCst) && Instant::now() < end {
        stop_host(sock);
        let wait = Instant::now() + Duration::from_secs(1);
        while !done.load(Ordering::SeqCst) && Instant::now() < wait {
            thread::sleep(Duration::from_millis(20));
        }
    }
}

/// Append a line of non-JSON output, keeping about the last 2000 bytes once the
/// buffer passes 4000. The cut moves forward to a character boundary: a byte
/// offset inside a multi-byte character would panic the slice.
pub(crate) fn push_stderr(buf: &mut String, line: &str) {
    buf.push_str(line);
    buf.push('\n');
    if buf.len() > 4000 {
        let mut cut = buf.len() - 2000;
        while !buf.is_char_boundary(cut) {
            cut += 1;
        }
        buf.drain(..cut);
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
    /// Bytes of output seen so far, for the idle watcher.
    seen: Arc<AtomicU64>,
}

impl Feed {
    /// Output at an absolute journal offset: counted, then parsed.
    fn chunk(&mut self, offset: u64, b: &[u8]) {
        self.seen.fetch_add(b.len() as u64, Ordering::SeqCst);
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
            push_stderr(&mut self.stderr, l);
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
        let dir0 = dir.clone();
        let extra = spec.deadline_extra.clone();
        thread::spawn(move || {
            let end = Instant::now() + mason_step_host::registry::remaining_limit(d, &dir0);
            while Instant::now() < end + extra.get() {
                if done.load(Ordering::SeqCst) {
                    return;
                }
                thread::sleep(Duration::from_millis(20));
            }
            timed_out.store(true, Ordering::SeqCst);
            stop_host_until_done(&sock, &done);
        });
    }

    let seen = Arc::new(AtomicU64::new(0));
    let idled = Arc::new(AtomicBool::new(false));
    if let Some(p) = spec
        .idle
        .clone()
        .filter(|p| !p.alert_after.is_zero() || !p.stop_after.is_zero())
    {
        let (dir, sock, seen, idled, done) = (
            dir.clone(),
            sock.clone(),
            seen.clone(),
            idled.clone(),
            done.clone(),
        );
        let contract = spec.contract.clone();
        thread::spawn(move || watch_idle(&p, &dir, &sock, &seen, &idled, &done, contract));
    }

    let accepted = spec.accepted.clone();
    {
        let progress = spec.progress.clone();
        let on_tokens = spec.on_tokens.clone();
        let (sid, cancelled, timed_out, done, idled) = (
            session_id.clone(),
            cancelled.clone(),
            timed_out.clone(),
            done.clone(),
            idled.clone(),
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
                seen,
            };
            let exit = follow(&dir, &sock, &mut feed);
            feed.flush();
            done.store(true, Ordering::SeqCst);
            let outcome = if timed_out.load(Ordering::SeqCst) {
                Outcome::Timeout
            } else if cancelled.load(Ordering::SeqCst) {
                Outcome::NoResult
            } else if idled.load(Ordering::SeqCst) {
                Outcome::HostLost {
                    detail: silent_stop(&dir)
                        .unwrap_or_else(|| "stopped after going silent".into()),
                }
            } else {
                match exit {
                    Some(rec) => decide(
                        &feed.st,
                        accepted.is_set(),
                        (rec.signal == 0).then_some(rec.code),
                        &feed.stderr,
                    ),
                    None => Outcome::HostLost {
                        detail: "the step host was lost before the agent finished".into(),
                    },
                }
            };
            let _ = tx.send(AgentEvent::Finished(outcome));
        });
    }

    let sock2 = h.root.join(&id).join("sock");
    let cancel_flag = cancelled.clone();
    let done2 = done.clone();
    let canceller: Canceller = Arc::new(move || {
        cancel_flag.store(true, Ordering::SeqCst);
        // First try here; if it does not get through, keep trying off the caller's thread.
        if !stop_host(&sock2) {
            let (sock, done) = (sock2.clone(), done2.clone());
            thread::spawn(move || stop_host_until_done(&sock, &done));
        }
    });
    Box::new(Session {
        rx,
        session_id,
        canceller,
        finished: false,
    })
}

/// Watch for silence: no new output and no contract call in flight. Past the
/// alert limit it reports the silence; past the stop limit it leaves a note in the
/// host's directory and stops the host. Ends when the session does.
fn watch_idle(
    p: &mason_adapter_api::IdlePolicy,
    dir: &Path,
    sock: &Path,
    seen: &AtomicU64,
    idled: &AtomicBool,
    done: &AtomicBool,
    contract: Option<mason_adapter_api::ContractHandle>,
) {
    let limits: Vec<Duration> = [p.alert_after, p.stop_after]
        .into_iter()
        .filter(|d| !d.is_zero())
        .collect();
    let tick = limits.iter().min().map_or(Duration::from_secs(1), |m| {
        (*m / 20).clamp(Duration::from_millis(5), Duration::from_secs(1))
    });
    let mut quiet_since = Instant::now();
    let mut last = seen.load(Ordering::SeqCst);
    let mut alerted = false;
    while !done.load(Ordering::SeqCst) {
        thread::sleep(tick);
        let now = seen.load(Ordering::SeqCst);
        let busy = contract.as_ref().is_some_and(|c| c.call_in_flight());
        if now != last || busy {
            last = now;
            quiet_since = Instant::now();
            if alerted {
                alerted = false;
                (p.report)(None);
            }
            continue;
        }
        let quiet = quiet_since.elapsed();
        if !p.stop_after.is_zero() && quiet >= p.stop_after {
            let note = format!(
                "the session was stopped after {} of silence (no output, no tool call in flight)",
                human(quiet)
            );
            let _ = std::fs::write(dir.join(SILENT_MARK), &note);
            idled.store(true, Ordering::SeqCst);
            if alerted {
                (p.report)(None);
            }
            stop_host(sock);
            return;
        }
        if !p.alert_after.is_zero() && quiet >= p.alert_after {
            alerted = true;
            (p.report)(Some(quiet.as_millis() as u64));
        }
    }
    if alerted {
        (p.report)(None);
    }
}

fn human(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 120 {
        format!("{} min", s / 60)
    } else {
        format!("{s} s")
    }
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

#[cfg(test)]
mod stderr_tests {
    use super::push_stderr;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn the_cut_keeps_the_buffer_valid_and_bounded(
            lines in proptest::collection::vec("[a-zé€😀 ]{0,300}", 1..60)
        ) {
            let mut buf = String::new();
            for l in &lines {
                push_stderr(&mut buf, l);
                prop_assert!(buf.len() <= 4000 + 4 + 1 + 1200);
            }
        }
    }
}
