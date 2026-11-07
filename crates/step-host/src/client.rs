// SPDX-License-Identifier: GPL-3.0-or-later

//! The controller's side: connect to a host, attach from a byte offset, and
//! fall back to the files on disk when the host is already gone.

use crate::protocol::{self, Frame, ProtoError};
use crate::registry::{self, ExitRecord};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;

#[derive(Debug)]
pub enum ClientError {
    Io(io::Error),
    Proto(ProtoError),
    /// The host refused the Hello (no common protocol version).
    Rejected(String),
    /// The host is gone without an exit record; carries the bytes salvaged.
    HostLost {
        collected: u64,
    },
}

impl From<io::Error> for ClientError {
    fn from(e: io::Error) -> Self {
        ClientError::Io(e)
    }
}

impl From<ProtoError> for ClientError {
    fn from(e: ProtoError) -> Self {
        ClientError::Proto(e)
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ClientError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Data { offset: u64, bytes: Vec<u8> },
    Exit(ExitRecord),
    Heartbeat { seq: u64, at_ms: u64 },
}

pub struct HostClient {
    stream: UnixStream,
    pub version: u16,
    pub host_pid: u32,
    pub child_pid: u32,
}

impl HostClient {
    /// Offer protocol versions `min..=max` (use [`protocol::PREVIOUS`] and
    /// [`protocol::CURRENT`] normally).
    pub fn connect(sock: &Path, min: u16, max: u16, name: &str) -> Result<HostClient, ClientError> {
        let mut stream = UnixStream::connect(sock)?;
        protocol::write_frame(
            &mut stream,
            &Frame::Hello {
                min,
                max,
                name: name.into(),
            },
        )?;
        match protocol::read_frame(&mut stream)? {
            Frame::HelloAck {
                version,
                host_pid,
                child_pid,
            } => Ok(HostClient {
                stream,
                version,
                host_pid,
                child_pid,
            }),
            Frame::Reject { reason } => Err(ClientError::Rejected(reason)),
            f => Err(ClientError::Proto(ProtoError::Bad(format!(
                "unexpected {f:?}"
            )))),
        }
    }

    pub fn attach(&mut self, offset: u64) -> Result<(), ClientError> {
        Ok(protocol::write_frame(
            &mut self.stream,
            &Frame::Attach { offset },
        )?)
    }

    pub fn stop(&mut self, grace_ms: u32) -> Result<(), ClientError> {
        Ok(protocol::write_frame(
            &mut self.stream,
            &Frame::Stop { grace_ms },
        )?)
    }

    /// Tell the host the result is safely collected, so it can go away.
    pub fn release(&mut self) -> Result<(), ClientError> {
        Ok(protocol::write_frame(&mut self.stream, &Frame::Release)?)
    }

    pub fn next_event(&mut self) -> Result<Event, ClientError> {
        match protocol::read_frame(&mut self.stream)? {
            Frame::Data { offset, bytes } => Ok(Event::Data { offset, bytes }),
            Frame::Exit {
                code,
                signal,
                output_len,
            } => Ok(Event::Exit(ExitRecord {
                code,
                signal,
                output_len,
                ended_ms: 0,
            })),
            Frame::Heartbeat { seq, at_ms } => Ok(Event::Heartbeat { seq, at_ms }),
            Frame::Reject { reason } => Err(ClientError::Rejected(reason)),
            f => Err(ClientError::Proto(ProtoError::Bad(format!(
                "unexpected {f:?}"
            )))),
        }
    }
}

fn write_exit(path: &Path, rec: &ExitRecord) -> io::Result<()> {
    registry::write_kv(
        path,
        &[
            ("code", rec.code.to_string()),
            ("signal", rec.signal.to_string()),
        ],
    )
}

/// Bring `received` up to date with the host's output and report how the
/// child ended. Resumes from `received`'s current length, so it can be called
/// again after the controller itself was killed at any point. Writes the exit
/// status to `exit_path` once everything is in `received`.
///
/// Uses the socket when the host is up, and the files on disk when it isn't.
pub fn collect_to_file(
    host_dir: &Path,
    received: &Path,
    exit_path: &Path,
) -> Result<ExitRecord, ClientError> {
    collect_to_file_observed(host_dir, received, exit_path, &mut |_, _| {})
}

/// [`collect_to_file`], calling `observe(offset, bytes)` for every chunk as it
/// is written (from the live stream or from the journal on disk), so a caller
/// can count the output as it arrives without reading the file again.
pub fn collect_to_file_observed(
    host_dir: &Path,
    received: &Path,
    exit_path: &Path,
    observe: &mut dyn FnMut(u64, &[u8]),
) -> Result<ExitRecord, ClientError> {
    let mut out = OpenOptions::new()
        .create(true)
        .append(true)
        .open(received)?;
    let mut have = fs::metadata(received)?.len();
    let info = registry::HostInfo::read(host_dir)?;

    let live = HostClient::connect(
        &info.sock,
        protocol::PREVIOUS,
        protocol::CURRENT,
        "step-host-ctl",
    );
    if let Ok(mut c) = live {
        c.attach(have)?;
        loop {
            match c.next_event() {
                Ok(Event::Data { offset, bytes }) => {
                    if offset != have {
                        return Err(
                            ProtoError::Bad(format!("gap: wanted {have}, got {offset}")).into()
                        );
                    }
                    out.write_all(&bytes)?;
                    observe(have, &bytes);
                    have += bytes.len() as u64;
                }
                Ok(Event::Heartbeat { .. }) => {}
                Ok(Event::Exit(mut rec)) => {
                    if have != rec.output_len {
                        return Err(ProtoError::Bad(format!(
                            "exit at {} but have {have}",
                            rec.output_len
                        ))
                        .into());
                    }
                    rec.ended_ms = ExitRecord::read(host_dir).map_or(0, |r| r.ended_ms);
                    out.sync_all()?;
                    write_exit(exit_path, &rec)?;
                    let _ = c.release();
                    return Ok(rec);
                }
                // The host went away mid-stream: fall through to the disk path.
                Err(ClientError::Proto(ProtoError::Io(_))) => break,
                Err(e) => return Err(e),
            }
        }
    }

    // Host not reachable: read what it left on disk.
    match ExitRecord::read(host_dir) {
        Ok(rec) => {
            while have < rec.output_len {
                let want = ((rec.output_len - have) as usize).min(protocol::DATA_CHUNK);
                let bytes = registry::read_journal(host_dir, have, want)?;
                if bytes.is_empty() {
                    return Err(ClientError::Io(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "journal shorter than exit record",
                    )));
                }
                out.write_all(&bytes)?;
                observe(have, &bytes);
                have += bytes.len() as u64;
            }
            out.sync_all()?;
            write_exit(exit_path, &rec)?;
            Ok(rec)
        }
        Err(_) => {
            let before = have;
            loop {
                let bytes = registry::read_journal(host_dir, have, protocol::DATA_CHUNK)?;
                if bytes.is_empty() {
                    break;
                }
                out.write_all(&bytes)?;
                observe(have, &bytes);
                have += bytes.len() as u64;
            }
            Err(ClientError::HostLost {
                collected: have - before,
            })
        }
    }
}
