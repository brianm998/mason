// SPDX-License-Identifier: GPL-3.0-or-later

//! A blocking client for the protocol's WebSocket: what the command line, the
//! conformance runner and the tests use. It speaks either encoding.

use crate::*;
use std::net::TcpStream;
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

pub struct Client {
    ws: WebSocket<TcpStream>,
    enc: Encoding,
    next_id: u64,
}

fn io(e: impl std::fmt::Display) -> String {
    e.to_string()
}

impl Client {
    pub fn connect(port: u16, enc: Encoding) -> Result<Client, String> {
        let stream = TcpStream::connect(("127.0.0.1", port))
            .map_err(|e| format!("cannot reach masond on 127.0.0.1:{port}: {e}"))?;
        let _ = stream.set_nodelay(true);
        let (ws, _) =
            tungstenite::client(format!("ws://127.0.0.1:{port}/ws"), stream).map_err(io)?;
        Ok(Client {
            ws,
            enc,
            next_id: 1,
        })
    }

    fn timeout(&mut self, d: Option<Duration>) {
        let _ = self.ws.get_mut().set_read_timeout(d);
    }

    pub fn send(&mut self, m: &ClientMessage) -> Result<(), String> {
        let bytes = encode_client(m, self.enc);
        let msg = match self.enc {
            Encoding::Protobuf => Message::Binary(bytes.into()),
            Encoding::Json => Message::Text(String::from_utf8_lossy(&bytes).to_string().into()),
        };
        self.ws.send(msg).map_err(io)
    }

    /// The next message, or `None` if nothing came within `wait`.
    pub fn recv(&mut self, wait: Duration) -> Result<Option<ServerMessage>, String> {
        self.timeout(Some(wait.max(Duration::from_millis(1))));
        loop {
            match self.ws.read() {
                Ok(Message::Binary(b)) => return decode_server(&b, Encoding::Protobuf).map(Some),
                Ok(Message::Text(t)) => {
                    return decode_server(t.as_bytes(), Encoding::Json).map(Some)
                }
                Ok(Message::Close(_)) => return Err("the daemon closed the connection".into()),
                Ok(_) => continue,
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(io(e)),
            }
        }
    }

    /// Say hello with the last sequence number seen; returns the welcome.
    pub fn hello(&mut self, last_seq: u64, commands_only: bool) -> Result<Welcome, String> {
        self.hello_as(PROTOCOL, last_seq, commands_only)
    }

    /// Say hello as a client that speaks `protocol` (for testing the version rules).
    pub fn hello_as(
        &mut self,
        protocol: u32,
        last_seq: u64,
        commands_only: bool,
    ) -> Result<Welcome, String> {
        self.send(&ClientMessage {
            body: Some(client_message::Body::Hello(Hello {
                protocol,
                last_seq,
                client: "mason-client".into(),
                token: String::new(),
                commands_only,
                client_version: "dev".into(),
            })),
        })?;
        match self.recv(Duration::from_secs(10))? {
            Some(ServerMessage {
                body: Some(server_message::Body::Welcome(w)),
            }) => Ok(w),
            Some(ServerMessage {
                body: Some(server_message::Body::Error(e)),
            }) => Err(e.message),
            other => Err(format!("expected a welcome, got {other:?}")),
        }
    }

    pub fn subscribe(
        &mut self,
        views: &[ViewKind],
        project: &str,
        task: u64,
    ) -> Result<(), String> {
        self.send(&ClientMessage {
            body: Some(client_message::Body::Subscribe(Subscribe {
                views: views.iter().map(|v| *v as i32).collect(),
                project: project.into(),
                task,
            })),
        })
    }

    /// Send a command and wait for its result, setting aside whatever else
    /// arrives meanwhile (returned in order).
    pub fn command(
        &mut self,
        mut c: Command,
        wait: Duration,
    ) -> Result<(CommandResult, Vec<ServerMessage>), String> {
        if c.id.is_empty() {
            c.id = format!("c{}", self.next_id);
            self.next_id += 1;
        }
        let id = c.id.clone();
        self.send(&ClientMessage {
            body: Some(client_message::Body::Command(c)),
        })?;
        let end = Instant::now() + wait;
        let mut other = vec![];
        loop {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err("the daemon did not answer in time".into());
            }
            match self.recv(left)? {
                Some(ServerMessage {
                    body: Some(server_message::Body::Result(r)),
                }) if r.id == id => return Ok((r, other)),
                Some(m) => other.push(m),
                None => {}
            }
        }
    }

    pub fn close(mut self) {
        let _ = self.ws.close(None);
    }
}
