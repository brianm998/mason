# The plugin protocol (version 3)

A plugin is a folder in `.mason/extensions/<name>/` on the **base branch** with a `manifest.toml` and a program. The program speaks JSON-RPC 2.0 over stdio, one JSON object per line, in any language. `docs/examples/plugins/migration-check/` is a complete example in Python.

## The manifest

```toml
name = "migration-check"          # must equal the folder's name
version = "1.0.0"
protocol = 3                      # required; this version speaks 1, 2 and 3
provides = ["step-type:migration-check"]
run = "./check.py"                # started in the folder; arguments are split on spaces
config_schema = "schema.json"     # optional: JSON Schema for the step's settings
categories = ["testing", "review"] # where the step may sit (default developing, testing)
```

Unknown keys are mistakes. `provides` may also list (protocol 3 only) `adapter:tracker:<name>`, `adapter:host:<name>`, `adapter:ci:<name>`, `parser:<name>` and `environment:<name>`; a protocol 1 or 2 manifest that lists one is refused, with that reason, and the rest of the project loads. `adapter:agent:<name>` is accepted in a manifest but nothing loads it yet (see decision 0286).

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

Protocols 2 and 3 are additive. A manifest says `protocol = 1` or `protocol = 2`; the host sends that number in `initialize` and the plugin answers with the same number (an answer with another is refused). A version 1 plugin works as before and never sends `step.progress`: if it does, the notification is ignored, and the conformance kit fails it for not declaring protocol 2.

## Cancel (protocol 3)

When the task is canceled while `step.run` is open, the host sends one `step.cancel` notification (`{"jsonrpc":"2.0","method":"step.cancel","params":{}}`, no `id`, no answer). The plugin stops what it is doing and answers `step.run` (with `passed: false`, say). The answer is not a verdict: the task is canceled. A plugin that does not stop is killed when the step's timeout ends, as before. A protocol 1 or 2 plugin is never sent it.

## Adapters, parsers and environments (protocol 3)

Every method's params and result are JSON objects. A JSON-RPC error answer's `code` says what failed: -32001 not found, -32002 invalid (refused for good), -32003 not allowed, -32004 slow down, anything else an outage to retry. A plugin is named by `kind = "plugin:<name>"` in the project file's `[host]`, `[ci]` or `[tracker]`, by `parser = "plugin:<name>"` in a suite, and by `type = "plugin:<name>"` in an environment; MASON finds the extension on the base branch that provides `adapter:<kind>:<name>`, `parser:<name>` or `environment:<name>`, and builds it only if the owner approved that exact hash (otherwise the service reports "needs approval" and nothing runs).

| Provides | Methods |
| --- | --- |
| `adapter:tracker:<n>` | `tracker.capabilities {}` -> `{conditional_polling, comments, labels, webhooks, dependencies}`; `tracker.list {etag?}` -> `{items: [{id, number, title, body, priority, labels, author, open, is_pr}], etag?}` or `{not_modified: true}`; `tracker.mirror {item, note}`; `tracker.close {item, note}`; `tracker.blocked_by {number}` -> `{blockers: [{number, open}]}`; `tracker.set_priority {number, priority}` |
| `adapter:host:<n>` | `host.capabilities {}` -> `{pull_requests, labels, checks, merge_queue}`; `host.open_pr {base, head, title, body}` -> `{number, url}`; `host.pr_status {number}` -> `{state, checks, failed, review, head}` (state open, merged or closed; checks pending, green or red; review satisfied, required or changes_requested); `host.merge_pr`, `host.enqueue`, `host.close_pr` `{number}` |
| `adapter:ci:<n>` | `ci.capabilities {}` -> `{arbitrary_ref, per_job_results}`; `ci.trigger {ref, sha}` -> `{run}`; `ci.status {run}` -> `{state, failed, summary}` (state pending, green or red); `ci.doctor {repo}` -> `{problems: [string]}` |
| `parser:<n>` | `parser.parse {text}` -> `{tests: [{name, passed?, status?, detail?}]}` (status pass, fail or skip). The verdict is judged from these cases exactly as for a built-in parser. |
| `environment:<n>` | `environment.setup`, `environment.health`, `environment.teardown` `{config}` -> `{ok, detail?}`. Health is asked again until it says ok or `health_timeout` is spent. |

The tracker, host and CI kits in `crates/adapter-api/src/kit.rs` run against these wrappers (`mason_plugins::adapters`) unchanged.

## Schema

`config_schema` uses: `type`, `enum`, `required`, `properties`, `additionalProperties: false`, `items`, `minimum`, `maximum`, `minLength`. Titles, descriptions, defaults and `x-*` hints are for forms. MASON checks a step's settings against it when the definition is validated and again before the plugin runs; the editor builds the step's form from it.

## Trust

An extension runs only if the owner approved its content hash (`mason extension approve <name>`). The hash is SHA-256 over every file's relative path, executable bit and bytes. Approvals are in `<config dir>/trust/<project>.toml`; the exact bytes that were hashed are copied to `<config dir>/extensions/<hash>/` and run from there. Extensions from a task branch, a worktree or a pull request are never read.

## Conformance

A plugin that provides no step type has no `step.run` to try: its `run`, `progress` and `refuses bad config` checks pass as "not applicable", and its methods are exercised by the adapter kits.


`conformance plugin-kit <dir> [--timeout SECS]` checks: the manifest, the schema file, that the program starts, `initialize` (shape, name, protocol and `provides` equal the manifest), that the schema it reports equals the file, that `step.run` with schema-valid settings answers `{passed, summary}` in time, that every `step.progress` it sent is well formed (and only a protocol 2 plugin sends any), that settings the schema forbids are refused, and that `shutdown` is answered and the process exits. Each failing check is named.
