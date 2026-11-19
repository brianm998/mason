# 0653: Adapter socket tests are Unix-only until the IPC port

Decision: `crates/adapters/tests/{codex,conformance,mcp}.rs` are gated with `#![cfg(unix)]`.

Why: the Windows CI job runs `cargo test -p mason-adapters`, and these binaries serve the
contract on an AF_UNIX socket (`ContractHandle::serve`) or run a step host. On Windows
`mason-platform` returns `Unsupported` there until the IPC transport lands, so they failed.

Lost: Windows coverage of the conformance and MCP bridge tests until the port.
