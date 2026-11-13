// SPDX-License-Identifier: GPL-3.0-or-later

//! A deliberate stop of the daemon: SIGTERM (launchd, `kickstart -k`) or SIGINT
//! (ctrl-C). The handler only stores the signal and its sender's pid in atomics,
//! which is all a signal handler may safely do; `wait` polls them from the event
//! loop. Without a handler the default action killed the process with nothing logged.

#[cfg(unix)]
use nix::libc;
#[cfg(unix)]
use nix::sys::signal::{sigaction, SaFlags, SigAction, SigHandler, SigSet, Signal};
use std::sync::atomic::{AtomicI32, Ordering};

static GOT: AtomicI32 = AtomicI32::new(0);
static FROM: AtomicI32 = AtomicI32::new(0);

#[cfg(unix)]
#[allow(unsafe_code)]
extern "C" fn on_signal(sig: libc::c_int, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
    let pid = if info.is_null() { 0 } else { sender(info) };
    FROM.store(pid, Ordering::SeqCst);
    GOT.store(sig, Ordering::SeqCst);
}

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn sender(info: *mut libc::siginfo_t) -> i32 {
    // SAFETY: the kernel passes a valid siginfo_t to an SA_SIGINFO handler.
    unsafe { (*info).si_pid }
}

#[cfg(all(unix, not(target_os = "macos")))]
#[allow(unsafe_code)]
fn sender(info: *mut libc::siginfo_t) -> i32 {
    // SAFETY: the kernel passes a valid siginfo_t to an SA_SIGINFO handler.
    unsafe { (*info).si_pid() }
}

/// Install the handlers for SIGTERM and SIGINT.
#[cfg(unix)]
#[allow(unsafe_code)]
pub fn install() -> nix::Result<()> {
    let act = SigAction::new(
        SigHandler::SigAction(on_signal),
        SaFlags::SA_SIGINFO | SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    for s in [Signal::SIGTERM, Signal::SIGINT] {
        // SAFETY: the handler only stores into atomics.
        unsafe { sigaction(s, &act) }?;
    }
    Ok(())
}

/// Windows has no SIGTERM; a stop request there arrives as a service control
/// event or a console ctrl-C, which the Windows service port task will handle.
#[cfg(windows)]
pub fn install() -> std::io::Result<()> {
    Err(mason_platform::unsupported(
        "handling a stop request",
        "service",
    ))
}

/// SIGTERM and SIGINT, which are the same numbers on macOS and Linux.
const SIGTERM: i32 = 15;
const SIGINT: i32 = 2;

/// The text for the log line: `SIGTERM from pid 1`, or just `SIGTERM` when the
/// sender is not known.
pub fn describe(sig: i32, pid: i32) -> String {
    let name = match sig {
        s if s == SIGTERM => "SIGTERM".to_string(),
        s if s == SIGINT => "SIGINT".to_string(),
        s => format!("signal {s}"),
    };
    if pid > 0 {
        format!("{name} from pid {pid}")
    } else {
        name
    }
}

/// Completes when one of the signals has arrived; says which and from whom.
pub async fn wait() -> String {
    loop {
        let sig = GOT.load(Ordering::SeqCst);
        if sig != 0 {
            return describe(sig, FROM.load(Ordering::SeqCst));
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_text_names_the_signal_and_the_sender_when_known() {
        assert_eq!(describe(SIGTERM, 1), "SIGTERM from pid 1");
        assert_eq!(describe(SIGINT, 0), "SIGINT");
    }
}
