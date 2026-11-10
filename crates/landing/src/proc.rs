// SPDX-License-Identifier: GPL-3.0-or-later

//! Child processes with a deadline. A child runs in its own process group, so on
//! expiry the whole group (git's helpers, a build's compilers) is killed, and the
//! caller learns it was the deadline and not the process that ended it.

use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum ProcError {
    /// The program could not be started.
    Spawn(std::io::Error),
    /// The deadline passed; the process group was killed. What it had said so far.
    TimedOut { stdout: Vec<u8>, stderr: Vec<u8> },
}

type Buf = Arc<Mutex<Vec<u8>>>;

fn drain<R: Read + Send + 'static>(mut r: R) -> (Buf, std::thread::JoinHandle<()>) {
    let buf: Buf = Arc::default();
    let b = buf.clone();
    let h = std::thread::spawn(move || {
        let mut chunk = [0u8; 8192];
        while let Ok(n) = r.read(&mut chunk) {
            if n == 0 {
                break;
            }
            b.lock()
                .unwrap_or_else(|e| e.into_inner())
                .extend_from_slice(&chunk[..n]);
        }
    });
    (buf, h)
}

fn take(b: &Buf) -> Vec<u8> {
    std::mem::take(&mut *b.lock().unwrap_or_else(|e| e.into_inner()))
}

fn kill_group(pid: u32) {
    let _ = nix::sys::signal::killpg(
        nix::unistd::Pid::from_raw(pid as i32),
        nix::sys::signal::Signal::SIGKILL,
    );
}

/// Run `cmd` to its end or `limit`, whichever comes first. Stdin is closed.
pub fn output(cmd: &mut Command, limit: Duration) -> Result<Output, ProcError> {
    use std::os::unix::process::CommandExt;
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(ProcError::Spawn)?;
    let pid = child.id();
    let (out, h1) = drain(child.stdout.take().expect("piped"));
    let (err, h2) = drain(child.stderr.take().expect("piped"));
    let end = Instant::now() + limit;
    let mut nap = Duration::from_millis(1);
    let status = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) if Instant::now() >= end => break None,
            Ok(None) => {
                std::thread::sleep(nap);
                nap = (nap * 2).min(Duration::from_millis(25));
            }
            Err(_) => break None,
        }
    };
    let Some(status) = status else {
        kill_group(pid);
        // Reap in the background: a process frozen at launch only dies once the
        // launch lets go of it, and the caller must not wait for that.
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        return Err(ProcError::TimedOut {
            stdout: take(&out),
            stderr: take(&err),
        });
    };
    // The pipes close when the process and its helpers are gone; a helper that
    // outlives it must not hold us up for ever.
    let settle = Instant::now() + Duration::from_secs(2);
    while !(h1.is_finished() && h2.is_finished()) && Instant::now() < settle {
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(Output {
        status,
        stdout: take(&out),
        stderr: take(&err),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_and_times_out() {
        let o = output(
            Command::new("sh").args(["-c", "echo hi; echo no >&2"]),
            Duration::from_secs(10),
        )
        .unwrap();
        assert_eq!(String::from_utf8_lossy(&o.stdout), "hi\n");
        assert_eq!(String::from_utf8_lossy(&o.stderr), "no\n");
        let t = Instant::now();
        let e = output(
            Command::new("sh").args(["-c", "echo started; sleep 30 & sleep 30"]),
            Duration::from_millis(300),
        );
        assert!(t.elapsed() < Duration::from_secs(3));
        match e {
            Err(ProcError::TimedOut { stdout, .. }) => assert_eq!(stdout, b"started\n"),
            other => panic!("{other:?}"),
        }
    }
}
