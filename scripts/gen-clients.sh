#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Generate the client code from schemas/mason.proto and clients/tokens.toml.
#   Rust:   automatic, by crates/protocol/build.rs (prost + pbjson, no protoc needed).
#   Swift:  swift-protobuf, if `protoc` and `protoc-gen-swift` are installed.
#   Kotlin: Wire, by the Gradle build of clients/compose/core (no separate step).
#   Tokens: always, for both clients.
# What cannot be generated here is reported, never silently skipped.
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"

cargo run -q -p mason-conformance -- tokens --write

if command -v protoc >/dev/null && command -v protoc-gen-swift >/dev/null; then
  mkdir -p clients/swift/Sources/MasonProtocol
  protoc -I schemas --swift_out=clients/swift/Sources/MasonProtocol --swift_opt=Visibility=Public schemas/mason.proto
  f=clients/swift/Sources/MasonProtocol/mason.pb.swift
  { echo "// SPDX-License-Identifier: GPL-3.0-or-later"; cat "$f"; } > "$f.tmp" && mv "$f.tmp" "$f"
  echo "wrote $f"
else
  echo "Swift: protoc and protoc-gen-swift are not both installed; not generated" >&2
fi

# Kotlin: Wire generates the messages as part of the Gradle build (clients/compose/core),
# straight from schemas/mason.proto; nothing to run here.
