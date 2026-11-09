// SPDX-License-Identifier: GPL-3.0-or-later

//! Background loops that survive a panic. A panic on a thread nobody joins ends that
//! thread silently, and with it the sweep, the alerts or the tracker polls; the daemon
//! goes on answering and looks healthy. Every long-lived loop is started here: each pass
//! runs under `catch_unwind`, a panic is filed as a `thread_panic` bug and logged, and
//! the loop goes on after a short pause.

use crate::daemon::Daemon;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Weak};
use std::time::Duration;

/// The pause after a panic, so a pass that always panics does not spin.
pub const PAUSE: Duration = Duration::from_millis(500);

/// What a panic said, as text.
pub fn message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "(no message)".into())
}

/// File and log a panic that was caught on `what`.
pub fn report(d: &Daemon, what: &str, p: &(dyn std::any::Any + Send)) {
    let msg = message(p);
    tracing::error!(thread = what, panic = %msg, "a thread panicked; it goes on");
    d.record_bug("thread_panic", &format!("{what} panicked: {msg}"));
}

/// Start the loop `name`. `pass` is called again and again with a handle on the daemon
/// (it sleeps itself, so a pass never keeps the daemon alive while it waits) and returns
/// `false` when the loop should end (the daemon is gone). Injected panic point:
/// `loop:<name>`.
pub fn spawn_loop<F>(d: &Arc<Daemon>, name: impl Into<String>, mut pass: F)
where
    F: FnMut(&Weak<Daemon>) -> bool + Send + 'static,
{
    let name: String = name.into();
    let weak = Arc::downgrade(d);
    let n = name.clone();
    let _ = std::thread::Builder::new().name(name).spawn(move || loop {
        let r = catch_unwind(AssertUnwindSafe(|| {
            if let Some(d) = weak.upgrade() {
                d.crash.panic_point(&format!("loop:{n}"));
            }
            pass(&weak)
        }));
        match r {
            Ok(true) => {}
            Ok(false) => break,
            Err(p) => {
                let Some(d) = weak.upgrade() else { break };
                report(&d, &format!("thread {n}"), p.as_ref());
                drop(d);
                std::thread::sleep(PAUSE);
            }
        }
    });
}
