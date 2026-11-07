# The plugin protocol (version 1)

A plugin is a folder in `.mason/extensions/<name>/` on the **base branch** with a `manifest.toml` and a program. The program speaks JSON-RPC 2.0 over stdio, one JSON object per line, in any language. `docs/examples/plugins/migration-check/` is a complete example in Python.

## The manifest

```toml
name = "migration-check"          # must equal the folder's name
version = "1.0.0"
protocol = 1                      # required; this version speaks 1
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

`config` is the step's settings (every key of the step the built-in steps do not use). A plugin must refuse settings its schema forbids with a JSON-RPC error. `env` holds the same `MASON_*` variables script steps get. `passed: false` sends the task back like a red test, with `summary` as the reason.

## Schema

`config_schema` uses: `type`, `enum`, `required`, `properties`, `additionalProperties: false`, `items`, `minimum`, `maximum`, `minLength`. Titles, descriptions, defaults and `x-*` hints are for forms. MASON checks a step's settings against it when the definition is validated and again before the plugin runs; the editor builds the step's form from it.

## Trust

An extension runs only if the owner approved its content hash (`mason extension approve <name>`). The hash is SHA-256 over every file's relative path, executable bit and bytes. Approvals are in `<config dir>/trust/<project>.toml`; the exact bytes that were hashed are copied to `<config dir>/extensions/<hash>/` and run from there. Extensions from a task branch, a worktree or a pull request are never read.

## Conformance

`conformance plugin-kit <dir> [--timeout SECS]` checks: the manifest, the schema file, that the program starts, `initialize` (shape, name, protocol and `provides` equal the manifest), that the schema it reports equals the file, that `step.run` with schema-valid settings answers `{passed, summary}` in time, that settings the schema forbids are refused, and that `shutdown` is answered and the process exits. Each failing check is named.
