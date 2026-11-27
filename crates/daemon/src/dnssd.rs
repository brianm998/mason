// SPDX-License-Identifier: GPL-3.0-or-later

//! Registering `_mason._tcp` with the system's own mDNS responder (macOS).
//!
//! A second responder publishing A/AAAA records for the Mac's own `.local` name makes
//! mDNSResponder see a name conflict and rename the machine. Going through `DNSServiceRegister`
//! with a null host and domain means the system answers for its own name and the SRV record
//! points at it. The call is made through a small seam (`Backend`) so tests need no live
//! responder.

use std::collections::BTreeMap;

/// What is handed to `DNSServiceRegister`. `host` and `domain` are always `None`: the system
/// fills in the Mac's real, current `.local` name and the default domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegisterCall {
    pub name: String,
    pub regtype: String,
    pub domain: Option<String>,
    pub host: Option<String>,
    pub port: u16,
    pub txt: Vec<u8>,
}

/// Where a registration is sent.
pub trait Backend {
    type Handle;
    fn register(&self, call: &RegisterCall) -> Result<Self::Handle, String>;
}

/// The wire TXT record: each entry is a length byte then `key=value`.
pub fn encode_txt(txt: &BTreeMap<String, String>) -> Vec<u8> {
    let mut out = Vec::new();
    for (k, v) in txt {
        let e = format!("{k}={v}");
        let b = e.as_bytes();
        let n = b.len().min(255);
        out.push(n as u8);
        out.extend_from_slice(&b[..n]);
    }
    if out.is_empty() {
        out.push(0);
    }
    out
}

/// Build the call (no host, no domain) and hand it to the backend. `regtype` has no `.local.`.
pub fn register_with<B: Backend>(
    b: &B,
    instance: &str,
    regtype: &str,
    port: u16,
    txt: &BTreeMap<String, String>,
) -> Result<B::Handle, String> {
    b.register(&RegisterCall {
        name: instance.to_string(),
        regtype: regtype
            .trim_end_matches('.')
            .trim_end_matches(".local")
            .to_string(),
        domain: None,
        host: None,
        port,
        txt: encode_txt(txt),
    })
}

#[cfg(target_os = "macos")]
pub use sys::{register, Registration};

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod sys {
    use super::{Backend, RegisterCall};
    use std::collections::BTreeMap;
    use std::ffi::{c_char, c_int, c_void, CString};
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    type Ref = *mut c_void;
    type Callback =
        extern "C" fn(Ref, u32, i32, *const c_char, *const c_char, *const c_char, *mut c_void);

    extern "C" {
        fn DNSServiceRegister(
            sd_ref: *mut Ref,
            flags: u32,
            interface_index: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port: u16,
            txt_len: u16,
            txt: *const c_void,
            callback: Option<Callback>,
            context: *mut c_void,
        ) -> i32;
        fn DNSServiceRefSockFD(sd_ref: Ref) -> c_int;
        fn DNSServiceProcessResult(sd_ref: Ref) -> i32;
    }

    #[derive(Default)]
    struct Shared {
        // None until the first callback; then the outcome.
        result: Mutex<Option<Result<(), String>>>,
        cv: Condvar,
    }

    extern "C" fn on_registered(
        _r: Ref,
        _flags: u32,
        err: i32,
        _name: *const c_char,
        _regtype: *const c_char,
        _domain: *const c_char,
        ctx: *mut c_void,
    ) {
        // SAFETY: `ctx` is the `Arc<Shared>` raw pointer leaked in `register`, never freed.
        let s = unsafe { &*(ctx as *const Shared) };
        let out = if err == 0 {
            Ok(())
        } else {
            Err(format!("DNSServiceRegister failed (error {err})"))
        };
        if let Err(e) = &out {
            tracing::warn!("mDNS registration: {e}");
        }
        *s.result.lock().unwrap_or_else(|e| e.into_inner()) = Some(out);
        s.cv.notify_all();
    }

    /// A live registration. Kept for the life of the process.
    pub struct Registration {
        _sd: Ref,
    }
    // SAFETY: the ref is only used by its own service thread.
    unsafe impl Send for Registration {}
    unsafe impl Sync for Registration {}

    struct System;

    struct SendRef(Ref);
    // SAFETY: handed to exactly one thread.
    unsafe impl Send for SendRef {}

    impl Backend for System {
        type Handle = Registration;
        fn register(&self, c: &RegisterCall) -> Result<Registration, String> {
            let cs = |s: &str| CString::new(s).map_err(|e| e.to_string());
            let (name, regtype) = (cs(&c.name)?, cs(&c.regtype)?);
            let domain = c.domain.as_deref().map(cs).transpose()?;
            let host = c.host.as_deref().map(cs).transpose()?;
            let ptr = |o: &Option<CString>| o.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
            let shared = Arc::new(Shared::default());
            let ctx = Arc::into_raw(shared.clone()) as *mut c_void;
            let mut sd: Ref = std::ptr::null_mut();
            let txt_len = u16::try_from(c.txt.len()).map_err(|_| "TXT record too long")?;
            // SAFETY: every pointer outlives the call; the system copies what it keeps.
            let rc = unsafe {
                DNSServiceRegister(
                    &mut sd,
                    0,
                    0,
                    name.as_ptr(),
                    regtype.as_ptr(),
                    ptr(&domain),
                    ptr(&host),
                    c.port.to_be(),
                    txt_len,
                    c.txt.as_ptr() as *const c_void,
                    Some(on_registered),
                    ctx,
                )
            };
            if rc != 0 {
                return Err(format!("DNSServiceRegister failed (error {rc})"));
            }
            let sdr = SendRef(sd);
            std::thread::Builder::new()
                .name("mdns-service".into())
                .spawn(move || {
                    let sdr = sdr;
                    // SAFETY: the ref is never deallocated, so it is valid for the process.
                    unsafe {
                        if DNSServiceRefSockFD(sdr.0) < 0 {
                            return;
                        }
                        // Blocks until the responder has something to say; runs the callback.
                        while DNSServiceProcessResult(sdr.0) == 0 {}
                    }
                    tracing::warn!("mDNS: the connection to the system responder closed");
                })
                .map_err(|e| e.to_string())?;
            // Wait on the callback (bounded) so a refusal reaches `node doctor`.
            let g = shared.result.lock().unwrap_or_else(|e| e.into_inner());
            let (g, _) = shared
                .cv
                .wait_timeout_while(g, Duration::from_secs(5), |r| r.is_none())
                .unwrap_or_else(|e| e.into_inner());
            if let Some(Err(e)) = g.as_ref() {
                return Err(e.clone());
            }
            Ok(Registration { _sd: sd })
        }
    }

    /// Register `_mason._tcp` through the system responder.
    pub fn register(
        instance: &str,
        port: u16,
        txt: &BTreeMap<String, String>,
    ) -> Result<Registration, String> {
        super::register_with(&System, instance, "_mason._tcp", port, txt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct Fake(RefCell<Vec<RegisterCall>>);
    impl Backend for Fake {
        type Handle = ();
        fn register(&self, c: &RegisterCall) -> Result<(), String> {
            self.0.borrow_mut().push(c.clone());
            Ok(())
        }
    }

    #[test]
    fn registration_passes_a_null_host_and_domain() {
        let f = Fake::default();
        let txt = BTreeMap::from([("fp".to_string(), "ab".to_string())]);
        register_with(&f, "MASON node on x", "_mason._tcp.local.", 7425, &txt).unwrap();
        let calls = f.0.borrow();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].host, None);
        assert_eq!(calls[0].domain, None);
        assert_eq!(calls[0].regtype, "_mason._tcp");
        assert_eq!(calls[0].port, 7425);
        assert_eq!(calls[0].txt, b"\x05fp=ab".to_vec());
    }
}
