// SPDX-License-Identifier: GPL-3.0-or-later

//! `snapshot_probe PORT`: connect as a monitor, say hello, and print how many
//! views the first snapshot holds (`snapshot views=N`). Used by the exit scripts.
//! Exits 1 if no snapshot arrives within 20 seconds.

use mason_protocol::client::Client;
use mason_protocol::{server_message, Encoding};
use std::time::{Duration, Instant};

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .expect("usage: snapshot_probe PORT");
    let mut c = Client::connect(port, Encoding::Protobuf).expect("connect");
    c.hello(0, false).expect("welcome");
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        match c.recv(Duration::from_millis(200)) {
            Ok(Some(m)) => {
                if let Some(server_message::Body::Snapshot(s)) = m.body {
                    println!("snapshot views={}", s.views.len());
                    return;
                }
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("closed: {e}");
                std::process::exit(1);
            }
        }
    }
    eprintln!("no snapshot");
    std::process::exit(1);
}
