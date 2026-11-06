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
pub const PROTOCOL: u32 = 1;

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
    fn a_hello_round_trips_in_both_encodings() {
        let m = ClientMessage {
            body: Some(client_message::Body::Hello(Hello {
                protocol: PROTOCOL,
                last_seq: 41,
                client: "test".into(),
                token: String::new(),
                commands_only: false,
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
}
