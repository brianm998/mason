// SPDX-License-Identifier: GPL-3.0-or-later

//! Starting a host from the daemon.

use crate::registry::HostInfo;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct SpawnOptions {
    pub host_bin: PathBuf,
    pub root: PathBuf,
    pub id: String,
    pub cmd: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(String, String)>,
    pub clear_env: bool,
    pub stdin: Option<PathBuf>,
    pub linger: Duration,
    pub heartbeat: Duration,
}

impl SpawnOptions {
    pub fn new(
        host_bin: impl Into<PathBuf>,
        root: impl Into<PathBuf>,
        id: &str,
        cmd: &[&str],
    ) -> Self {
        SpawnOptions {
            host_bin: host_bin.into(),
            root: root.into(),
            id: id.into(),
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            cwd: None,
            env: vec![],
            clear_env: false,
            stdin: None,
            linger: Duration::from_secs(600),
            heartbeat: Duration::from_secs(1),
        }
    }
}

/// Start a host and return once it is up (its record is on disk and its socket
/// is listening). The host outlives this process.
pub fn spawn_host(o: &SpawnOptions) -> io::Result<HostInfo> {
    let dir = o.root.join(&o.id);
    let mut cmd = Command::new(&o.host_bin);
    cmd.arg("--dir").arg(&dir).arg("--id").arg(&o.id);
    cmd.arg("--linger-ms").arg(o.linger.as_millis().to_string());
    cmd.arg("--hb-ms").arg(o.heartbeat.as_millis().to_string());
    if let Some(c) = &o.cwd {
        cmd.arg("--cwd").arg(c);
    }
    if o.clear_env {
        cmd.arg("--clear-env");
    }
    if let Some(p) = &o.stdin {
        cmd.arg("--stdin").arg(p);
    }
    for (k, v) in &o.env {
        cmd.arg("--env").arg(format!("{k}={v}"));
    }
    cmd.arg("--").args(&o.cmd);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = cmd.spawn()?;

    // Reap the host when it eventually exits, without blocking the caller.
    let gone = Arc::new(AtomicBool::new(false));
    {
        let gone = gone.clone();
        thread::spawn(move || {
            let _ = child.wait();
            gone.store(true, Ordering::SeqCst);
        });
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(info) = HostInfo::read(&dir) {
            if info.sock.exists() {
                return Ok(info);
            }
        }
        if gone.load(Ordering::SeqCst) {
            // It may have written its record and finished very quickly.
            if let Ok(info) = HostInfo::read(&dir) {
                return Ok(info);
            }
            return Err(io::Error::other("step host exited before it was up"));
        }
        if Instant::now() > deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "step host did not start",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
}
