// SPDX-License-Identifier: GPL-3.0-or-later

//! Compile `schemas/mason.proto` with `protox` (a protobuf compiler written in
//! Rust, so building MASON needs no `protoc`) and generate the prost types and
//! the proto3 JSON mapping the command line and debugging use.

use std::path::PathBuf;

fn main() {
    let schema = PathBuf::from("../../schemas/mason.proto");
    println!("cargo:rerun-if-changed={}", schema.display());
    let fds = protox::compile([&schema], ["../../schemas"]).expect("schemas/mason.proto compiles");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR"));
    let descriptor_path = out.join("mason_descriptor.bin");
    let mut config = prost_build::Config::new();
    config.file_descriptor_set_path(&descriptor_path);
    config
        .compile_fds(fds.clone())
        .expect("prost generates the messages");
    pbjson_build::Builder::new()
        .register_descriptors(&prost::Message::encode_to_vec(&fds))
        .expect("descriptors register")
        .build(&[".mason.v1"])
        .expect("pbjson generates the JSON mapping");
}
