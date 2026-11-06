// SPDX-License-Identifier: GPL-3.0-or-later

//! Wire protocol between a controller (the daemon) and a step host.
//!
//! Frame: `u32 BE length` of (`u8 type` + payload), then type and payload.
//! Version 1 is the baseline. Version 2 adds `Heartbeat` frames. A host
//! speaks the current and previous version, so the daemon and its hosts can be
//! updated at different times (Part 15).

use std::io::{self, Read, Write};

pub const CURRENT: u16 = 2;
pub const PREVIOUS: u16 = 1;
pub const MAX_FRAME: usize = 1 << 20;
pub const DATA_CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    // controller -> host
    Hello {
        min: u16,
        max: u16,
        name: String,
    },
    /// Start streaming output from this byte offset of the journal.
    Attach {
        offset: u64,
    },
    /// Terminate the child's process group, escalating after `grace_ms`.
    Stop {
        grace_ms: u32,
    },
    /// The controller has everything, including the exit status.
    Release,
    // host -> controller
    HelloAck {
        version: u16,
        host_pid: u32,
        child_pid: u32,
    },
    Reject {
        reason: String,
    },
    Data {
        offset: u64,
        bytes: Vec<u8>,
    },
    /// `code` is -1 when killed by a signal; `signal` is 0 when it exited.
    Exit {
        code: i32,
        signal: i32,
        output_len: u64,
    },
    /// Version 2 only.
    Heartbeat {
        seq: u64,
        at_ms: u64,
    },
}

#[derive(Debug)]
pub enum ProtoError {
    Io(io::Error),
    Bad(String),
}

impl From<io::Error> for ProtoError {
    fn from(e: io::Error) -> Self {
        ProtoError::Io(e)
    }
}

impl std::fmt::Display for ProtoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtoError::Io(e) => write!(f, "io: {e}"),
            ProtoError::Bad(m) => write!(f, "protocol: {m}"),
        }
    }
}

impl std::error::Error for ProtoError {}

/// Highest version both sides speak, among the ones this build supports.
pub fn negotiate(min: u16, max: u16) -> Option<u16> {
    [CURRENT, PREVIOUS]
        .into_iter()
        .find(|v| (min..=max).contains(v))
}

fn put_str(b: &mut Vec<u8>, s: &str) {
    b.extend_from_slice(&(s.len() as u32).to_be_bytes());
    b.extend_from_slice(s.as_bytes());
}

pub fn encode(f: &Frame) -> Vec<u8> {
    let mut b = vec![];
    match f {
        Frame::Hello { min, max, name } => {
            b.push(1);
            b.extend_from_slice(&min.to_be_bytes());
            b.extend_from_slice(&max.to_be_bytes());
            put_str(&mut b, name);
        }
        Frame::Attach { offset } => {
            b.push(2);
            b.extend_from_slice(&offset.to_be_bytes());
        }
        Frame::Stop { grace_ms } => {
            b.push(3);
            b.extend_from_slice(&grace_ms.to_be_bytes());
        }
        Frame::Release => b.push(4),
        Frame::HelloAck {
            version,
            host_pid,
            child_pid,
        } => {
            b.push(10);
            b.extend_from_slice(&version.to_be_bytes());
            b.extend_from_slice(&host_pid.to_be_bytes());
            b.extend_from_slice(&child_pid.to_be_bytes());
        }
        Frame::Reject { reason } => {
            b.push(11);
            put_str(&mut b, reason);
        }
        Frame::Data { offset, bytes } => {
            b.push(12);
            b.extend_from_slice(&offset.to_be_bytes());
            b.extend_from_slice(bytes);
        }
        Frame::Exit {
            code,
            signal,
            output_len,
        } => {
            b.push(13);
            b.extend_from_slice(&code.to_be_bytes());
            b.extend_from_slice(&signal.to_be_bytes());
            b.extend_from_slice(&output_len.to_be_bytes());
        }
        Frame::Heartbeat { seq, at_ms } => {
            b.push(14);
            b.extend_from_slice(&seq.to_be_bytes());
            b.extend_from_slice(&at_ms.to_be_bytes());
        }
    }
    let mut out = (b.len() as u32).to_be_bytes().to_vec();
    out.extend(b);
    out
}

/// One `write_all`, so only a crash can leave a partial frame on the wire.
pub fn write_frame(w: &mut impl Write, f: &Frame) -> io::Result<()> {
    w.write_all(&encode(f))?;
    w.flush()
}

struct Cur<'a>(&'a [u8]);

impl Cur<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], ProtoError> {
        if self.0.len() < n {
            return Err(ProtoError::Bad("short frame".into()));
        }
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        Ok(a)
    }
    fn u16(&mut self) -> Result<u16, ProtoError> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().expect("2 bytes"),
        ))
    }
    fn u32(&mut self) -> Result<u32, ProtoError> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn i32(&mut self) -> Result<i32, ProtoError> {
        Ok(i32::from_be_bytes(
            self.take(4)?.try_into().expect("4 bytes"),
        ))
    }
    fn u64(&mut self) -> Result<u64, ProtoError> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().expect("8 bytes"),
        ))
    }
    fn str(&mut self) -> Result<String, ProtoError> {
        let n = self.u32()? as usize;
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| ProtoError::Bad("bad utf8".into()))
    }
    fn rest(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0).to_vec()
    }
    fn end(&self) -> Result<(), ProtoError> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(ProtoError::Bad("trailing bytes".into()))
        }
    }
}

pub fn decode(buf: &[u8]) -> Result<Frame, ProtoError> {
    let mut c = Cur(buf);
    let ty = c.take(1)?[0];
    let f = match ty {
        1 => Frame::Hello {
            min: c.u16()?,
            max: c.u16()?,
            name: c.str()?,
        },
        2 => Frame::Attach { offset: c.u64()? },
        3 => Frame::Stop { grace_ms: c.u32()? },
        4 => Frame::Release,
        10 => Frame::HelloAck {
            version: c.u16()?,
            host_pid: c.u32()?,
            child_pid: c.u32()?,
        },
        11 => Frame::Reject { reason: c.str()? },
        12 => Frame::Data {
            offset: c.u64()?,
            bytes: c.rest(),
        },
        13 => Frame::Exit {
            code: c.i32()?,
            signal: c.i32()?,
            output_len: c.u64()?,
        },
        14 => Frame::Heartbeat {
            seq: c.u64()?,
            at_ms: c.u64()?,
        },
        t => return Err(ProtoError::Bad(format!("unknown frame type {t}"))),
    };
    c.end()?;
    Ok(f)
}

pub fn read_frame(r: &mut impl Read) -> Result<Frame, ProtoError> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(ProtoError::Bad(format!("bad frame length {len}")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    decode(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_round_trips() {
        let frames = vec![
            Frame::Hello {
                min: 1,
                max: 2,
                name: "daemon ü".into(),
            },
            Frame::Attach { offset: u64::MAX },
            Frame::Stop { grace_ms: 5000 },
            Frame::Release,
            Frame::HelloAck {
                version: 2,
                host_pid: 7,
                child_pid: 9,
            },
            Frame::Reject {
                reason: "no".into(),
            },
            Frame::Data {
                offset: 3,
                bytes: vec![0, 255, 10, 13],
            },
            Frame::Data {
                offset: 0,
                bytes: vec![],
            },
            Frame::Exit {
                code: -1,
                signal: 9,
                output_len: 123,
            },
            Frame::Heartbeat { seq: 4, at_ms: 5 },
        ];
        for f in frames {
            let bytes = encode(&f);
            assert_eq!(read_frame(&mut &bytes[..]).unwrap(), f);
        }
    }

    #[test]
    fn malformed_frames_are_errors_not_panics() {
        for bad in [
            &[0u8, 0, 0, 0][..],
            &[0, 0, 0, 1][..],
            &[0, 0, 0, 2, 99, 0][..],
            &[255, 255, 255, 255][..],
            &[0, 0, 0, 3, 2, 0, 0][..],
        ] {
            assert!(read_frame(&mut &bad[..]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn negotiation_picks_the_highest_common_version() {
        assert_eq!(negotiate(1, 2), Some(2));
        assert_eq!(negotiate(1, 1), Some(1));
        assert_eq!(negotiate(1, 9), Some(2));
        assert_eq!(negotiate(2, 2), Some(2));
        assert_eq!(negotiate(3, 4), None);
        assert_eq!(negotiate(0, 0), None);
    }
}
