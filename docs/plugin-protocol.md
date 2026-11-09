# The plugin protocol (version 2)

A plugin is a folder in `.mason/extensions/<name>/` on the **base branch** with a `manifest.toml` and a program. The program speaks JSON-RPC 2.0 over stdio, one JSON object per line, in any language. `docs/examples/plugins/migration-check/` is a complete example in Python.

## The manifest

```toml
name = "migration-check"          # must equal the folder's name
version = "1.0.0"
protocol = 2                      # required; this version speaks 1 and 2
provides = ["step-type:migration-check"]
run = "./check.py"                # started in the folder; arguments are split on spaces
config_schema = "schema.json"     # optional: JSON Schema for the step's settings
categories = ["testing", "review"] # where the step may sit (default developing, testing)
```

Unknown keys are mistakes. `parser:`, `environment:` and `adapter:` in `provides` are **not yet supported**: the extension is refused, with that reason, and the rest of the project loads.

## Messages

The host sends requests, the plugin answers each one with a result or an error. Anything the plugin writes to stderr is its log (`<data dir>/tasks/.../plugin-<step>-r<run>.log`), not protocol.

| Method | Params | Result |
| --- | --- | --- |
| `initialize` | `{protocol, host}` | `{protocol, name, version, provides, config_schema}` |
| `step.run` | `{step, type, config, task, title, worktree, env}` | `{passed, summary, data?}` |
| `shutdown` | `{}` | `null`, then the process exits |
| `step.progress` | `{done?, total?, unit?, message?}` | none: a notification the plugin sends (protocol 2) |

`config` is the step's settings (every key of the step the built-in steps do not use). A plugin must refuse settings its schema forbids with a JSON-RPC error. `env` holds the same `MASON_*` variables script steps get. `passed: false` sends the task back like a red test, with `summary` as the reason.

## Progress (protocol 2)

While `step.run` is open a plugin may send `step.progress` notifications: a JSON-RPC message with `"method": "step.progress"` and **no `id`**, which the host does not answer. `params` is an object with at least one of `done` and `total` (whole numbers), `unit` and `message` (strings), and nothing else. The host shows them on the step's live progress record: `done` and `total` as reported figures, `message` as the step's latest line. A malformed notification is ignored at run time (and flagged by the conformance kit); a notification does not extend the time `step.run` is allowed.

```json
{"jsonrpc": "2.0", "method": "step.progress", "params": {"done": 3, "total": 12, "unit": "migrations", "message": "003_users.sql"}}
```

### Versions and negotiation

Protocol 2 is additive. A manifest says `protocol = 1` or `protocol = 2`; the host sends that number in `initialize` and the plugin answers with the same number (an answer with another is refused). A version 1 plugin works as before and never sends `step.progress`: if it does, the notification is ignored, and the conformance kit fails it for not declaring protocol 2.

## Schema

`config_schema` uses: `type`, `enum`, `required`, `properties`, `additionalProperties: false`, `items`, `minimum`, `maximum`, `minLength`. Titles, descriptions, defaults and `x-*` hints are for forms. MASON checks a step's settings against it when the definition is validated and again before the plugin runs; the editor builds the step's form from it.

## Trust

An extension runs only if the owner approved its content hash (`mason extension approve <name>`). The hash is SHA-256 over every file's relative path, executable bit and bytes. Approvals are in `<config dir>/trust/<project>.toml`; the exact bytes that were hashed are copied to `<config dir>/extensions/<hash>/` and run from there. Extensions from a task branch, a worktree or a pull request are never read.

## Conformance

`conformance plugin-kit <dir> [--timeout SECS]` checks: the manifest, the schema file, that the program starts, `initialize` (shape, name, protocol and `provides` equal the manifest), that the schema it reports equals the file, that `step.run` with schema-valid settings answers `{passed, summary}` in time, that every `step.progress` it sent is well formed (and only a protocol 2 plugin sends any), that settings the schema forbids are refused, and that `shutdown` is answered and the process exits. Each failing check is named.
