# 0286: plugins can provide adapters, parsers and environments

**Decision.** Protocol 3 lets a plugin provide `adapter:tracker|host|ci:<name>`, `parser:<name>` and `environment:<name>` (and name `adapter:agent:<name>`, which nothing loads yet), and adds a `step.cancel` notification. The project file's `[host]`, `[ci]` and `[tracker]` accept `kind = "plugin:<name>"`; a suite accepts `parser = "plugin:<name>"`; an environment `type = "plugin:<name>"`. A plugin is built only when the extension's current content hash is approved, by the same `approved()` rule step plugins use.

**Why.** Before, a new tracker, host, CI system, parser or environment meant changing MASON's core. The plugin protocol refused every kind except step types.

**Design.**
- The wrappers (`PluginTracker`, `PluginHost`, `PluginCi` in `crates/plugins/src/adapters.rs`) implement the existing traits over JSON-RPC, so pollers, landing and the conformance kits take them unchanged. They live in `mason-plugins`, not in core, engine or scheduler, which stay free of I/O.
- `ParserKind` and `EnvKind` stay `Copy` enums: a suite gets `plugin_parser: Option<String>` and an environment `plugin: Option<String>` instead of a new enum variant, so no existing `match` or test changes. The verdict code is untouched: a parser plugin only returns cases (`parse::from_plugin`), and `judge` reads them as it reads a built-in parser's.
- Cancel: the wait in `Plugin::call_cancelable` looks at a cancel callback every 50 ms and sends `step.cancel` once. The daemon's callback reads the task's state. A canceled task's step does not conclude.
- `mason definition check --project P` loads P's approved extensions (read from the base commit, nothing runs) so plugin step types validate offline.
- An adapter's name is the name after `adapter:<kind>:`; the wrapper checks the plugin lists it in `initialize`.

**Deferred.** The agent adapter (`AgentAdapter`) is a streaming session with a cancel closure, which does not fit request and answer; it needs its own design. A manifest may name it so a plugin can be written ahead.

**Trust.** Protected paths touched, each needing the owner's approval: `crates/testing/**` (`model.rs` two new optional fields, `parse.rs` `from_plugin`; no verdict, gate or budget logic changes), `crates/daemon/src/suites.rs` (the parser and environment dispatch), and `scripts/exit/s292.sh` (the new exit test). Nothing is weakened; the refusal test in `manifest.rs` now checks the new rule (protocol 1 and 2 are still refused).
