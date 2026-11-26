// SPDX-License-Identifier: GPL-3.0-or-later

//! The client protocol (Part 16). The messages are generated from
//! `schemas/mason.proto`; this crate adds the two encodings a peer speaks
//! (protobuf binary and the proto3 JSON mapping) and the protocol's constants.

#![allow(clippy::all)]

pub mod v1 {
    include!(concat!(env!("OUT_DIR"), "/mason.v1.rs"));
    include!(concat!(env!("OUT_DIR"), "/mason.v1.serde.rs"));
}

pub use v1::*;

pub mod client;

use prost::Message;

/// The protocol version this build speaks.
pub const PROTOCOL: u32 = 2;

/// The oldest client protocol this build still serves: current and previous, so
/// that a daemon and its monitors need not update at the same moment.
pub const MIN_PROTOCOL: u32 = if PROTOCOL > 1 { PROTOCOL - 1 } else { 1 };

/// May a client that speaks `v` connect?
pub fn accepts(v: u32) -> bool {
    (MIN_PROTOCOL..=PROTOCOL).contains(&v)
}

/// Frames are binary protobuf, or text in the proto3 JSON mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    Protobuf,
    Json,
}

pub fn encode_server(m: &ServerMessage, e: Encoding) -> Vec<u8> {
    match e {
        Encoding::Protobuf => m.encode_to_vec(),
        Encoding::Json => serde_json::to_vec(m).expect("a message serializes"),
    }
}

pub fn decode_client(bytes: &[u8], e: Encoding) -> Result<ClientMessage, String> {
    match e {
        Encoding::Protobuf => ClientMessage::decode(bytes).map_err(|x| x.to_string()),
        Encoding::Json => serde_json::from_slice(bytes).map_err(|x| x.to_string()),
    }
}

pub fn encode_client(m: &ClientMessage, e: Encoding) -> Vec<u8> {
    match e {
        Encoding::Protobuf => m.encode_to_vec(),
        Encoding::Json => serde_json::to_vec(m).expect("a message serializes"),
    }
}

pub fn decode_server(bytes: &[u8], e: Encoding) -> Result<ServerMessage, String> {
    match e {
        Encoding::Protobuf => ServerMessage::decode(bytes).map_err(|x| x.to_string()),
        Encoding::Json => serde_json::from_slice(bytes).map_err(|x| x.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_and_previous_are_served_and_older_or_newer_are_not() {
        assert!(accepts(PROTOCOL));
        assert!(!accepts(0));
        assert!(!accepts(PROTOCOL + 1));
        assert!(MIN_PROTOCOL <= PROTOCOL && PROTOCOL - MIN_PROTOCOL <= 1);
    }

    #[test]
    fn a_hello_round_trips_in_both_encodings() {
        let m = ClientMessage {
            body: Some(client_message::Body::Hello(Hello {
                protocol: PROTOCOL,
                last_seq: 41,
                client: "test".into(),
                token: String::new(),
                commands_only: false,
                client_version: "abc123".into(),
                actor: "brian".into(),
            })),
        };
        for e in [Encoding::Protobuf, Encoding::Json] {
            let bytes = encode_client(&m, e);
            assert_eq!(decode_client(&bytes, e).unwrap(), m, "{e:?}");
        }
    }

    #[test]
    fn json_uses_the_protobuf_mapping() {
        let m = ServerMessage {
            body: Some(server_message::Body::Welcome(Welcome {
                protocol: 1,
                seq: 7,
                resume: ResumeKind::Events as i32,
                daemon: "masond".into(),
                demo: false,
                version: "dev".into(),
                min_protocol: MIN_PROTOCOL,
                owner: true,
                now_ms: 5,
                role: String::new(),
            })),
        };
        let text = String::from_utf8(encode_server(&m, Encoding::Json)).unwrap();
        // proto3 JSON: camelCase names, enums by name, 64-bit integers as strings.
        assert!(text.contains("\"welcome\""), "{text}");
        assert!(text.contains("\"resume\":\"EVENTS\""), "{text}");
        assert!(text.contains("\"seq\":\"7\""), "{text}");
    }

    #[test]
    fn a_peer_ignores_fields_it_does_not_know() {
        // A newer server adds field 99 to Pong: an older client still reads it.
        let mut bytes = Pong { nonce: 5 }.encode_to_vec();
        bytes.extend([0xF8, 0x06, 0x01]); // field 111, varint 1
        assert_eq!(Pong::decode(bytes.as_slice()).unwrap().nonce, 5);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode_client(&[0xFF, 0xFF, 0xFF], Encoding::Protobuf).is_err());
        assert!(decode_client(b"not json", Encoding::Json).is_err());
    }

    #[test]
    fn a_compile_row_carries_its_command_and_an_old_payload_lacks_it() {
        let w = CompileWaiter {
            label: "python3 - <<'EOF' …".into(),
            command: "python3 - <<'EOF'\nprint(1)\nEOF".into(),
            ..Default::default()
        };
        let h = CompileHolder {
            command: "cargo build".into(),
            ..Default::default()
        };
        for e in [Encoding::Protobuf, Encoding::Json] {
            let (bw, bh) = match e {
                Encoding::Protobuf => (w.encode_to_vec(), h.encode_to_vec()),
                Encoding::Json => (
                    serde_json::to_vec(&w).unwrap(),
                    serde_json::to_vec(&h).unwrap(),
                ),
            };
            let (dw, dh): (CompileWaiter, CompileHolder) = match e {
                Encoding::Protobuf => (
                    CompileWaiter::decode(&*bw).unwrap(),
                    CompileHolder::decode(&*bh).unwrap(),
                ),
                Encoding::Json => (
                    serde_json::from_slice(&bw).unwrap(),
                    serde_json::from_slice(&bh).unwrap(),
                ),
            };
            assert_eq!((dw, dh), (w.clone(), h.clone()), "{e:?}");
        }
        // A payload from a daemon before the field existed.
        let old: CompileWaiter = serde_json::from_str(r##"{"label":"#5","position":1}"##).unwrap();
        assert_eq!(old.command, "");
        let old: CompileHolder = serde_json::from_str(r##"{"label":"#5"}"##).unwrap();
        assert_eq!(old.command, "");
    }

    /// A home view and a task page as a client built before #1155 knew them (some fields).
    #[derive(Clone, PartialEq, prost::Message)]
    struct Before {
        #[prost(string, tag = "1")]
        status: String,
    }

    #[test]
    fn hosts_round_trip_and_an_older_client_skips_them() {
        let run = HostRun {
            task: Some(TaskRow {
                id: 5,
                state: "developing".into(),
                ..Default::default()
            }),
            kind: "check".into(),
            name: "unit".into(),
            host: "node:marmot".into(),
            since_ms: 7,
            url: String::new(),
        };
        let home = HomeView {
            status: "ok".into(),
            hosts: vec![HostView {
                kind: "node".into(),
                name: "marmot".into(),
                status: "accepting".into(),
                runs: vec![run.clone()],
            }],
            ..Default::default()
        };
        let bytes = prost::Message::encode_to_vec(&home);
        assert_eq!(
            <HomeView as prost::Message>::decode(&bytes[..]).unwrap(),
            home
        );
        let json = serde_json::to_vec(&home).unwrap();
        assert_eq!(serde_json::from_slice::<HomeView>(&json).unwrap(), home);
        let old = <Before as prost::Message>::decode(&bytes[..]).unwrap();
        assert_eq!(old.status, "ok");
        let detail = TaskDetailView {
            running_on: vec![run],
            ..Default::default()
        };
        let bytes = prost::Message::encode_to_vec(&detail);
        assert_eq!(
            <TaskDetailView as prost::Message>::decode(&bytes[..]).unwrap(),
            detail
        );
        // The new fields have numbers of their own: 16 on the home view, 25 on the task page.
        assert_eq!(
            &prost::Message::encode_to_vec(&HomeView {
                hosts: vec![HostView::default()],
                ..Default::default()
            })[..],
            [0x82, 0x01, 0x00]
        );
        assert_eq!(
            &prost::Message::encode_to_vec(&TaskDetailView {
                running_on: vec![HostRun::default()],
                ..Default::default()
            })[..],
            [0xca, 0x01, 0x00]
        );
    }
}
