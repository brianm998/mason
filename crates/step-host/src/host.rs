// SPDX-License-Identifier: GPL-3.0-or-later

//! The step host: one detached process per running step. It owns the child,
//! journals its output to a file, and serves controllers over a unix socket.
//! If the controller (the daemon) dies, the host keeps running and keeps
//! journaling; a new controller reattaches by byte offset.

use crate::protocol::{self, Frame, ProtoError, DATA_CHUNK};
use crate::registry::{now_ms, write_kv, ExitRecord};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::FileExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Longest socket path we accept (macOS limits `sun_path` to 104 bytes).
const MAX_SOCK_PATH: usize = 100;

#[derive(Clone, Debug)]
pub struct HostConfig {
    pub dir: PathBuf,
    pub id: String,
    pub cmd: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    /// How long to wait for a controller to collect the result after the
    /// child exits before the host goes away (files stay on disk).
    pub linger: Duration,
    pub heartbeat: Duration,
}

#[derive(Default)]
struct St {
    size: u64,
    eof: bool,
    exit: Option<ExitRecord>,
    release: bool,
}

struct Shared {
    m: Mutex<St>,
    cv: Condvar,
    cfg: HostConfig,
    child_pid: u32,
    journal_path: PathBuf,
}

fn lock(s: &Shared) -> std::sync::MutexGuard<'_, St> {
    s.m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn run(cfg: HostConfig) -> io::Result<()> {
    // Our own session: no controlling terminal, and the launcher's death or
    // terminal closing can't signal us. Fails harmlessly if already a leader.
    let _ = nix::unistd::setsid();

    fs::create_dir_all(&cfg.dir)?;
    let sock_path = cfg.dir.join("sock");
    if sock_path.as_os_str().len() > MAX_SOCK_PATH {
        return Err(io::Error::other(format!(
            "socket path too long: {}",
            sock_path.display()
        )));
    }
    let journal_path = cfg.dir.join("journal");
    let journal = OpenOptions::new()
        .create_new(true)
        .append(true)
        .open(&journal_path)?;
    let _ = fs::remove_file(&sock_path);
    let listener = UnixListener::bind(&sock_path)?;
    listener.set_nonblocking(true)?;

    let (reader, writer) = io::pipe()?;
    let mut cmd = Command::new(&cfg.cmd[0]);
    cmd.args(&cfg.cmd[1..])
        .stdin(Stdio::null())
        .stdout(writer.try_clone()?)
        .stderr(writer)
        .process_group(0);
    if let Some(c) = &cfg.cwd {
        cmd.current_dir(c);
    }
    cmd.envs(cfg.env.iter().map(|(k, v)| (k, v)));
    let spawned = cmd.spawn();
    drop(cmd); // closes our copies of the pipe's write end

    let mut initial_len = 0u64;
    let (child, child_pid) = match spawned {
        Ok(c) => {
            let pid = c.id();
            (Some(c), pid)
        }
        Err(e) => {
            let msg = format!("step-host: failed to start {:?}: {e}\n", cfg.cmd);
            (&journal).write_all(msg.as_bytes())?;
            initial_len = msg.len() as u64;
            (None, 0)
        }
    };

    let shared = Arc::new(Shared {
        m: Mutex::new(St {
            size: initial_len,
            ..St::default()
        }),
        cv: Condvar::new(),
        cfg: cfg.clone(),
        child_pid,
        journal_path: journal_path.clone(),
    });

    // First heartbeat before the record appears, so anyone who finds the host
    // also finds a heartbeat.
    write_heartbeat(&cfg);
    write_kv(
        &cfg.dir.join("host.kv"),
        &[
            ("id", cfg.id.clone()),
            ("host_pid", std::process::id().to_string()),
            ("child_pid", child_pid.to_string()),
            ("sock", sock_path.display().to_string()),
            ("started_ms", now_ms().to_string()),
            ("heartbeat_ms", cfg.heartbeat.as_millis().to_string()),
        ],
    )?;

    // Pump: child output -> journal, then wake streamers.
    {
        let s = shared.clone();
        let mut journal = journal.try_clone()?;
        let mut reader = reader;
        thread::spawn(move || {
            let mut buf = vec![0u8; DATA_CHUNK];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if journal.write_all(&buf[..n]).is_err() {
                            break;
                        }
                        lock(&s).size += n as u64;
                        s.cv.notify_all();
                    }
                }
            }
            lock(&s).eof = true;
            s.cv.notify_all();
        });
    }

    // Waiter: record the exit once the child is gone and its output drained.
    {
        let s = shared.clone();
        let journal = journal.try_clone()?;
        thread::spawn(move || {
            let (code, signal) = match child {
                Some(mut c) => match c.wait() {
                    Ok(st) => (st.code().unwrap_or(-1), st.signal().unwrap_or(0)),
                    Err(_) => (-1, 0),
                },
                None => (127, 0),
            };
            // Grandchildren holding the pipe open must not delay the record.
            let deadline = Instant::now() + Duration::from_secs(2);
            let mut g = lock(&s);
            while !g.eof {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                g =
                    s.cv.wait_timeout(g, left)
                        .unwrap_or_else(|e| e.into_inner())
                        .0;
            }
            let rec = ExitRecord {
                code,
                signal,
                output_len: g.size,
                ended_ms: now_ms(),
            };
            let _ = journal.sync_all();
            let _ = rec.write(&s.cfg.dir);
            g.exit = Some(rec);
            drop(g);
            s.cv.notify_all();
        });
    }

    // Heartbeat file, so a daemon that restarts can see this host is alive.
    {
        let cfg = cfg.clone();
        thread::spawn(move || loop {
            thread::sleep(cfg.heartbeat);
            write_heartbeat(&cfg);
        });
    }

    // Accept loop; ends when the result has been collected or lingered out.
    let mut exited_at: Option<Instant> = None;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                let s = shared.clone();
                thread::spawn(move || {
                    let _ = serve(stream, &s);
                });
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(20))
            }
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
        let g = lock(&shared);
        if g.exit.is_some() {
            let since = *exited_at.get_or_insert_with(Instant::now);
            if g.release || since.elapsed() >= cfg.linger {
                break;
            }
        }
    }
    let _ = fs::remove_file(&sock_path);
    Ok(())
}

fn write_heartbeat(cfg: &HostConfig) {
    let tmp = cfg.dir.join("hb.tmp");
    if fs::write(&tmp, now_ms().to_string()).is_ok() {
        let _ = fs::rename(&tmp, cfg.dir.join("hb"));
    }
}

fn stop_child(s: &Shared, grace_ms: u32) {
    if s.child_pid == 0 {
        return;
    }
    let pg = Pid::from_raw(s.child_pid as i32);
    let _ = killpg(pg, Signal::SIGTERM);
    let deadline = Instant::now() + Duration::from_millis(u64::from(grace_ms));
    let mut g = lock(s);
    while g.exit.is_none() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            let _ = killpg(pg, Signal::SIGKILL);
            return;
        }
        g =
            s.cv.wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
    }
    // The child is gone; clean up anything it left in its group.
    let _ = killpg(pg, Signal::SIGKILL);
}

fn serve(mut stream: UnixStream, s: &Arc<Shared>) -> Result<(), ProtoError> {
    // BSD and macOS let an accepted socket inherit the listener's O_NONBLOCK.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let version = match protocol::read_frame(&mut stream)? {
        Frame::Hello { min, max, .. } => match protocol::negotiate(min, max) {
            Some(v) => v,
            None => {
                let reason = format!(
                    "no common protocol version: host speaks {}..={}, controller offered {min}..={max}",
                    protocol::PREVIOUS,
                    protocol::CURRENT
                );
                protocol::write_frame(&mut stream, &Frame::Reject { reason })?;
                return Ok(());
            }
        },
        _ => return Err(ProtoError::Bad("expected Hello".into())),
    };
    protocol::write_frame(
        &mut stream,
        &Frame::HelloAck {
            version,
            host_pid: std::process::id(),
            child_pid: s.child_pid,
        },
    )?;

    // Reader thread: Attach / Stop / Release arrive here, in any order.
    stream.set_read_timeout(None)?;
    let mut rd = stream.try_clone()?;
    let (tx, rx) = channel::<u64>();
    let closed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let s = s.clone();
        let closed = closed.clone();
        thread::spawn(move || {
            while let Ok(f) = protocol::read_frame(&mut rd) {
                match f {
                    Frame::Attach { offset } => {
                        let _ = tx.send(offset);
                    }
                    Frame::Stop { grace_ms } => {
                        let s = s.clone();
                        thread::spawn(move || stop_child(&s, grace_ms));
                    }
                    Frame::Release => {
                        lock(&s).release = true;
                        s.cv.notify_all();
                    }
                    _ => break,
                }
            }
            closed.store(true, std::sync::atomic::Ordering::SeqCst);
            s.cv.notify_all();
        });
    }

    let mut off = match rx.recv_timeout(Duration::from_secs(10)) {
        Ok(o) => o,
        Err(RecvTimeoutError::Timeout) => return Ok(()),
        Err(RecvTimeoutError::Disconnected) => return Ok(()),
    };
    let journal = File::open(&s.journal_path)?;
    if off > lock(s).size {
        protocol::write_frame(
            &mut stream,
            &Frame::Reject {
                reason: "offset beyond journal".into(),
            },
        )?;
        return Ok(());
    }

    let hb = s.cfg.heartbeat;
    let mut seq = 0u64;
    let mut last_hb = Instant::now();
    let mut exit_sent = false;
    loop {
        // Decide what to send under the lock, send outside it.
        let mut g = lock(s);
        let next = loop {
            if closed.load(std::sync::atomic::Ordering::SeqCst) {
                return Ok(());
            }
            if g.size > off {
                break Some(Frame::Data {
                    offset: off,
                    bytes: vec![],
                });
            }
            if let (Some(rec), false) = (&g.exit, exit_sent) {
                break Some(Frame::Exit {
                    code: rec.code,
                    signal: rec.signal,
                    output_len: rec.output_len,
                });
            }
            if version >= 2 && last_hb.elapsed() >= hb {
                break None;
            }
            let wait = if version >= 2 {
                hb.saturating_sub(last_hb.elapsed())
            } else {
                Duration::from_millis(500)
            };
            g =
                s.cv.wait_timeout(g, wait.max(Duration::from_millis(5)))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
        };
        let size = g.size;
        drop(g);
        match next {
            Some(Frame::Data { .. }) => {
                let n = ((size - off) as usize).min(DATA_CHUNK);
                let mut buf = vec![0u8; n];
                journal.read_exact_at(&mut buf, off)?;
                protocol::write_frame(
                    &mut stream,
                    &Frame::Data {
                        offset: off,
                        bytes: buf,
                    },
                )?;
                off += n as u64;
            }
            Some(exit) => {
                protocol::write_frame(&mut stream, &exit)?;
                exit_sent = true;
            }
            None => {
                seq += 1;
                protocol::write_frame(
                    &mut stream,
                    &Frame::Heartbeat {
                        seq,
                        at_ms: now_ms(),
                    },
                )?;
                last_hb = Instant::now();
            }
        }
    }
}
