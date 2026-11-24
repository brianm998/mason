<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Plan: running MASON work on other Macs ("nodes")

Status: **reviewed by the owner on 2026-11-23.** The owner's answers are built into this text
and listed in §10. The protected paths in §8 are approved. Nothing is left open.

This plan is for an agent working **outside the MASON queue**, in a worktree, on its own branch.
All file:line references are to `origin/develop` at `540d59118`. Re-read each one with
`git show origin/develop:<path>` before you act on it, because develop moves.

---

## 1. Goal

The iMac Pro that runs `masond` is failing and overheats under load. GitHub Actions remote runs
help, but they queue, start cold, and use up the API budget. A Mac Studio is on the same desk.

What we want:

- Any `masond` can **serve** as a *node*: it accepts compile and test jobs from another
  `masond` (the *coordinator*), runs them on its own CPUs, and sends back exactly what a local
  run would have produced.
- The coordinator **finds** nodes on the LAN with mDNS and **pairs** with each one once, using a
  code. From then on, **the coordinator's own identity is the credential**: mutual TLS with the
  coordinator's existing certificate. A node keeps **no tokens and no auth configuration**. The
  only thing it records is which coordinator(s) it trusts, and pairing writes that, not a person.
- **A node has almost no local configuration.** Its tunables (job count, CPU limit, lease, and
  so on) are set on the coordinator and sent to the node with each status poll. Locally a node
  has only `[node] serve = true`, the owner's on/off switch, and the trusted-coordinator list
  that pairing writes.
- **One landing queue per project, on the coordinator.** The coordinator does every git step
  of landing (merging, resolving conflicts, promoting, pushing). It may send the *test run* of
  a merged batch, and of its bisect halves, to nodes. Nodes never merge, never push, and never
  hold push credentials (§3.13).
- Each node **reports whether it can take work**, based on its CPU, its heat, its free slots, and
  an **owner on/off switch** ("accept work from other Macs"). The Studio is the owner's work
  machine, so the switch is the "working hours" control. Turning it off takes the machine back
  at once.
- A node needs **no project configuration**. Everything a job needs (the code snapshot, the
  command, the environment, the limits) comes from the coordinator with the job. The node keeps
  only caches (a git mirror and warm build directories) so later jobs are fast.
- Placement gets a third kind of executor. Today there are **Local** and **GitHub**. We add
  **Node**, and make the code shaped so that more kinds can be added later.

Out of scope for this plan (see §9): running agent (LLM) sessions on nodes, simulators and
devices on nodes, splitting shards across nodes, schedule-based working hours, and non-macOS
nodes.

---

## 2. What already exists (build on it, don't rebuild it)

MASON already has most of the pieces. The design below is mostly about connecting them.

| Piece | Where | What it gives us |
|---|---|---|
| Executor seam | `crates/daemon/src/remote.rs:47` `enum Executor { Local, GitHub { runs_on } }`; `executor_for` :67, `executor_global` :93, `executor_task` :107 | The one place that says where a run goes. Callers get back a `Finished` (:174) that looks just like a local step host's result, so parsers, verdicts and evidence don't change. |
| Run description | `remote.rs:222` `RunSpec`; `run_json` :782 | Everything a run needs, taken from the **pinned definition**, never from the task's tree. This is the "config the coordinator passes to the node". |
| Snapshot | `remote.rs:862` `snapshot()` | A commit of exactly what a local run would see, including uncommitted files, built with a temporary index. It applies the trust overlay. |
| Remote run lifecycle | `remote.rs:1587` `execute()`; `run_or_fallback` :3314; `HoldGuard` :3193; `reap_adopted` :3141; record files | Record first, then push, dispatch, poll, collect, and clean up. Survives a daemon restart. `Fail::Environmental` (:199) sends the run to the local fallback. |
| What a runner does | `.github/workflows/mason-run.yml`, the `run` step | Reads `.mason/run.json`, exports its env, runs `build` then `command` with `sh -c`, tars `results_file`, writes `result.json {code, signal, build_s, run_s, phase}`, uploads `output.log`, `result.json` and `results/`. **The node runner is a Rust port of this step.** |
| Placement decision | `crates/daemon/src/place.rs:462` `decide()`, `balance()` :403, `Place` :360, `Mode` :31 (local / remote / favor-local / favor-remote), `LocalLoad` :130, `facts_for` :849 | A pure decision from facts, covered by table tests. Explains every outcome. |
| Call sites | `suites.rs:457` (suite runs) → `run_remote` :585 → `remote_ran` :723; `hostedcheck.rs:645-690` (`run_check`, `goes_remote`); `landing.rs:5230`; `covmap.rs:523` | The places that match on `Executor::GitHub`. Each of them needs a `Node` arm. |
| LAN listener | `crates/daemon/src/lan.rs`: TLS with a self-signed cert (`load_or_create_identity`), `start` :231, `router` :271, `guard` :306, pairing codes, bearer tokens kept only as SHA-256, revocation | Its TLS setup, mDNS advertising and the code-pairing logic (`Pairing`) are reused by a separate node listener (§3.3). The phone listener itself is unchanged. |
| mDNS | `lan.rs:31` `SERVICE_TYPE = "_mason._tcp.local."`, `txt_record` :173 (`proto`, `fp`, `version`), `advertise` :181; crate `mdns-sd 0.11` is already a dependency | Advertising already works. We add **browsing** (same crate) and a few TXT keys. |
| TLS identity | `lan.rs` `load_or_create_identity` (self-signed cert and key in `<data>/lan/`, made even when the listener is off), `tls_config` :~155 (`with_no_client_auth`) | The coordinator presents this cert as its **client certificate**. The node's listener uses the same identity as its server certificate. |
| Landing | `landing.rs` (module doc): a per-project queue, batches merged into an integration worktree, suites run once over the union, then the red ladder (rerun, blame, **bisect by halves**), then fast-forward promote under the repo lock; the "do all suites go remote" check at :5230 | This already happens on one machine per project. Only its test runs move to nodes (§3.13). |
| Load signals | `cpu.rs` (CPU %, mach ticks), `thermal.rs` (`pmset` speed limit, SMC die °C, power-limit; `hot` :139), `sched.rs:605` `LoadReading` (slots used, limits and waiters), `disk.rs` | What a node reports about itself. |
| Remote-run view | `remote_runs.rs` (the monitor's "GitHub" section) | We generalise it to "Remote runs" and add node rows. |
| Live switch pattern | `place.rs` `remote.toml` + `remote-changes.log`, edited with `toml_edit` and read live on every call | Copy this pattern for the node's accept switch. |

---

## 3. Design

### 3.1 Roles and processes

It is one binary and one daemon. A `masond` can play either role, or both:

- **Coordinator**: it has projects and runs the pipeline. If `[nodes] enabled = true`, it may
  send runs to paired nodes.
- **Node**: if `[node] serve = true`, it accepts jobs on its own node listener (§3.3). A node may have no
  projects at all. The Studio starts out like that: `masond` installed, `[node] serve = true`,
  nothing else.

Both machines run the **same MASON version**. If the node protocol versions differ, the
coordinator refuses to use the node and shows why (§3.9). Each machine updates itself as it does
today (`mason update`). Remote updating of nodes is deferred (§9).

### 3.2 Discovery (mDNS)

- **Node side.** When `[node] serve = true`, advertise the `_mason._tcp` service even if the
  phone LAN listener is off. Reuse `advertise` (`lan.rs:181`) with the existing keys (`proto`,
  `fp`, `version`) plus `node=1`, `nport=<node listener port>`, `node_proto=<N>`,
  `nid=<stable node id>` (a random id made once and stored in `<data>/node/id`, so that renaming
  the host doesn't look like a new node), and `arch=arm64|x86_64`. Load and accepting state are **not** put in TXT. TXT updates
  arrive late and unreliably, so live state comes from polling over HTTPS (§3.4).
- **Coordinator side.** A new `nodes.rs` runs an `mdns_sd::ServiceDaemon::browse(SERVICE_TYPE)`
  loop on its own thread. It keeps the records with `node=1`, keyed by `nid`, along with their
  addresses, port and `fp`. A discovered node is only *listed*. It is never used until it has
  been paired.
- **Static fallback.** `[nodes] static = ["studio.local:7425", "10.0.0.12:7425"]` covers the
  cases where multicast is blocked (VPN, Wi-Fi isolation), and tests use it too
  (`127.0.0.1:<port>`).

### 3.3 Pairing and authentication: mutual TLS, delegated by the coordinator

The owner asked that nodes store **no tokens and no auth configuration**, and that the
coordinator hand out trust. So there are no bearer tokens at all. **Each side's TLS certificate
is its identity.** Both daemons already have one (`load_or_create_identity`). The coordinator
presents its certificate as a TLS *client certificate*, and each side pins the other's SHA-256
fingerprint.

A node still has to remember *who* its coordinator is, because otherwise anyone on the LAN could
send it commands. That memory is one fingerprint per trusted coordinator, kept in
`<data>/node/coordinators.json`. Pairing writes it. It is state, like the store, not
configuration: nobody edits it, and it holds no secret. (A fingerprint is public. The
coordinator's private key never leaves the coordinator.)

**The node listener.** This is a separate listener, `node_serve.rs`, on port 7425 (advertised in
mDNS as `nport`), started when `[node] serve = true`. It does not depend on `[lan] enabled`. It
serves **only** `/node/*` and `/node-pair`. Its rustls `ServerConfig` **requires** a client
certificate. A custom `ClientCertVerifier` accepts any well-formed self-signed cert during the
handshake. A per-connection layer then reads `peer_certificates()` from the TLS stream and puts
the fingerprint in the request extensions. A guard middleware allows `/node/*` only when that
fingerprint is in `coordinators.json`. Keeping this listener apart from the phone and monitor
LAN listener means the phone pairing, the device tokens and `access::decide` stay exactly as
they are.

**Pairing (done once, by the owner, from `docs/nodes.md`):**

1. On the Studio: run `mason node pair-code`. It prints an eight-digit code, valid for 2
   minutes (the lifetime `Pairing` already has; the plan first said 10), and this node's fingerprint. Use the existing `mason_core::access::Pairing` for the
   code's lifetime, its attempt limit and its lockout.
2. On the iMac: run `mason node add [<name>|--host H:P]`. It lists the discovered nodes and asks
   for the code. The coordinator connects with its client cert, **checks that the node's server
   cert matches the `fp` in mDNS** (or the fingerprint the owner pastes for a static host), and
   sends `POST /node-pair {code, name}`.
3. The node checks the code and records `{fingerprint, name, paired_at}` of the coordinator's
   client cert in `coordinators.json`, written with `mason_platform::fs::owner_only`. The
   coordinator records `{nid, name, host, port, node fingerprint}` in a new store table, `nodes`.

**Every later request:** the coordinator's HTTPS client presents its cert and **pins** the node's
fingerprint, using a custom `ServerCertVerifier` that compares SHA-256(cert DER). The node's
guard checks the client fingerprint. Nothing secret is stored anywhere new.

**Revocation:** on the node, `mason node coordinators remove <name>`. On the coordinator,
`mason node remove <name>`. If the coordinator's certificate is ever replaced, pair again.

**Node settings come from the coordinator too** (§3.4, §4). A node has only built-in defaults
until a coordinator sends it settings.

**HTTP client.** The daemon has no HTTP client crate yet. Use `hyper` 1 (`client`, `http1`
features) on `tokio-rustls` (already dependencies) with the client cert and the pinning
verifier, in a new `node_client.rs`. Calls run on the blocking pool or as async tasks, never on the event loop in a
way that blocks it. If enabling features pulls in new crates, check them with `cargo deny check`.
`deny.toml` is protected, so ask the owner before you change it.

### 3.4 Availability: when a node takes work

A node answers `POST /node/status` (to a paired coordinator only). The request body carries that
coordinator's **settings for this node**: `max_jobs`, `max_other_cpu_pct`, `min_disk_free_gb`,
`on_switch_off`, `lease_s` and thermal limits. They come from `[nodes.<name>]` on the coordinator
(§4). The node keeps the last settings it received from each coordinator **in memory only**.
Until a coordinator has sent settings, the node uses safe built-in defaults (`max_jobs = 2`,
everything else as in §4). When there are several coordinators, the strictest value of each
setting applies.

```json
{
  "node_proto": 1, "version": "0.1.0+<sha>", "nid": "…", "name": "studio",
  "arch": "arm64", "os": "macOS 15.x", "cores": 24,
  "accepting": true, "why_not": null,
  "switch": {"on": true, "changed_by": "brian", "changed_at": 1763600000000},
  "cpu_pct": 23, "other_cpu_pct": 4,
  "thermal": {"limit_pct": 100, "die_c": 61.0, "power_limited_ms": 0, "hot": false},
  "slots": {"compile": {"free": 3, "limit": 4}, "test-suite": {"free": 2, "limit": 3}},
  "jobs": {"running": 1, "max": 4},
  "disk_free_gb": 410,
  "toolchains": {"rust": ["1.96.0"], "xcode": ["16.4"], "java": ["21"]},
  "projects": {"mason": {"mirror": true, "slots": 3}}
}
```

`accepting` is true only when **all** of these hold. `why_not` names the first one that fails,
in plain words ("the owner turned remote work off", "die 96 °C", "CPU 85 % from other work"):

1. **The switch is on.** It lives in `<config>/node.toml` (`accept = true|false`, `changed_by`,
   `changed_at`), is edited with `toml_edit`, is appended to `<config>/node-changes.log`, and is
   read live. This is the same pattern as `remote.toml` in `place.rs`. **The default is off.** A
   freshly installed node takes nothing until the owner turns it on.
2. **Not hot.** Use `thermal::hot` with the node's own thermal settings. *Apple Silicon check:*
   the SMC keys and `pmset -g therm` in `thermal.rs` were written for the Intel iMac Pro. On the
   Studio, check what they return. If they return nothing, add the macOS thermal pressure level
   (`notify_get_state` on `com.apple.system.thermalpressurelevel`: 0 nominal … 4 sleeping) as a
   source. Treat "heavy" or worse as hot. Read it with `/usr/bin/notifyutil -g com.apple.system.thermalpressurelevel`
   (present on macOS, no FFI and no `unsafe`; `thermal::pressure_level`). This Intel Mac returns
   a level, so the call works; what the Studio reports is checked by hand (phase 1, manual).
3. **The owner's own work leaves room.** `other_cpu_pct` is total CPU minus the CPU of the
   node's own job processes. Use the process-tree CPU accounting already in `cputasks.rs`. It
   must be under the coordinator-set `max_other_cpu_pct` (default 50). This is the "I'm working on it right
   now" signal, short of turning the switch off.
4. **A slot is free.** The node's own broker (`sched.rs`) could grant the slot the job needs
   (`compile`, or `compile` + `test-suite` for a build-and-test job) **right now**, under its
   usual CPU and thermal gates, and `jobs.running < max_jobs`.
5. **Disk.** At least `min_disk_free_gb` is free (`disk.rs`).
6. *(Optional, phase 5)* The keyboard and mouse have been idle for `idle_min` minutes
   (`ioreg -c IOHIDSystem` `HIDIdleTime`).

**Jobs don't queue on the node.** A node accepts a job only if it can start it at once. If not,
it answers `409 {why}`. Queuing stays on the coordinator, in rank order, so we never wait in two
queues.

**Turning the switch off** stops new jobs at once. By the owner's choice, **running jobs finish
by default** (`on_switch_off = "drain"`). `mason node status` and the monitor show "draining: 2
jobs left". The coordinator can set `on_switch_off = "cancel"` for a node. Then running jobs are
killed, the coordinator sees `cancelled_by_node` (`Fail::Environmental`), and it places them
elsewhere. Separately, `mason node stop-jobs` on the node cancels the running jobs immediately,
for the moment the owner needs the machine back right now.

### 3.5 Job protocol (node listener, paired coordinators only, all under `/node/`)

| Method + path | Purpose |
|---|---|
| `POST /node/status` (body: this coordinator's settings for the node) | §3.4. The coordinator polls it every `[nodes] status_every_s` (default 5). It is a POST because it carries the settings. |
| `GET /node/mirror/{project}/has?c=<sha>&c=<sha>…` | Which of the given commits the node's mirror already has. Used to choose a bundle base. |
| `PUT /node/mirror/{project}` (body: git bundle) | Adds objects to the node's bare mirror `<data>/node/mirrors/<coordinator nid>/<project>.git`. It is verified with `git bundle verify` and fetched into `refs/mason/…`. |
| `POST /node/jobs` `{id, project, sha, run_json, limit_s, lease_s}` | Start a job. `201 {job}`, or `409 {why}` (no room), or `412 {missing}` (sha not in the mirror). |
| `GET /node/jobs/{id}` | `{state: running\|done\|cancelled\|lost, started_ms, ended_ms, code?}`. **Each call renews the job's lease.** |
| `GET /node/jobs/{id}/log?from=<byte>` | Output so far, as raw bytes from the offset. This gives live counts (§3.8). |
| `GET /node/jobs/{id}/artifact` | `{result, results: [{path, b64}]}`: what `result.json` and `results/` of the GitHub artifact hold. **Built as JSON, not a tar** (the coordinator never extracts an archive from a node); `output.log` comes from `/log`. |
| `DELETE /node/jobs/{id}` | Cancel. |
| `POST /node/jobs/{id}/ack` | The result was collected, so the node may delete the job directory. |

Job ids are the coordinator's step host id (`spec.id`) prefixed with the coordinator's `nid`, so
two coordinators can never collide.

### 3.6 Getting the code there

The node has no GitHub credentials, and it doesn't need any.

1. The coordinator builds the snapshot commit with the existing `remote::snapshot()`, which
   applies the trust overlay. `.mason/run.json` is written from `run_json(spec)`. The workflow
   file is irrelevant to a node but harmless.
2. It asks `/has` about a few candidate bases: the snapshot's parents, then `origin/<base>`
   and the last few base commits it sent to this node (cached per node).
3. If the node has a base, the coordinator sends `git bundle create - <snapshot> ^<base>`, which
   is tiny. If the node has none (first contact, or the mirror was cleared), it first sends a
   full bundle of `origin/<base>`, then the delta.
4. After a `412` it re-sends with a full base. This happens at most once per job.

**Node checkouts ("slots").** These live at
`<data>/node/work/<coordinator nid>/<project>/<project>-slot-<k>`, one per concurrent job, and
are reused. Each job does `git worktree add --detach` (the first time) or
`git checkout --detach -f <sha>`, then `git clean -ffdx` **excluding the build output
directories**, so builds stay incremental. That warm cache is the main speed-up over GitHub.
MASON's suites build into `$HOME/.cache/mason-target/<checkout dir basename>`, so the basename
must be unique across projects. That is why it is `<project>-slot-<k>`. The node runs at most
`max_jobs` slots per project and takes the least recently used one that is free.

**GC.** Job directories are deleted on `ack`, or 24 h after they end if nobody collected them.
Mirrors get `git gc --auto` after N bundles. Slots unused for 14 days are removed.

### 3.7 What the node runs (the runner)

Write a new `node_run.rs` as a Rust port of the `run` step of `.github/workflows/mason-run.yml`.
It reads only the `run_json` sent with the job, never anything from the snapshot's tree.

- **Environment.** Clear it, then add the node's own base (`PATH`, `HOME`, `TMPDIR`,
  `CARGO_HOME`, `RUSTUP_HOME` and the like, from the node daemon), then `run_json.env`, then
  `CARGO_TERM_COLOR=never` and `NO_COLOR=1`. **Path-valued variables from the coordinator are
  never sent.** The coordinator's `CARGO_TARGET_DIR` names a disk the node doesn't have.
  *Phase 2 task:* list every variable a local suite run and a local `run_check` set
  (`suites.rs`, `hostedcheck.rs`, `with_site_env`), and sort each one as send / node sets its
  own / drop. GitHub's `MASON_*`-only filter (`run_json`, `remote.rs:785`) is a good start, but
  check what local runs depend on (the test-mode network guard, `MASON_TASK`, sccache settings).
- **Shims.** The node applies the **same `xcodebuild` and `caffeinate` shims** that are used
  under `MASON_TASK` (see `agentshim.rs` and the CLAUDE.md "Sharing the machine" section).
  **Nothing may ever appear on the Studio's screen**: no UI tests and no `caffeinate -u`. This
  matters even more on a work machine.
- **Run.** If `build` is set, run `sh -c "$build"`. If it succeeds, run `sh -c "$command"`. The
  combined output goes to `output.log`. Record `build_s` and `run_s`, and set `phase` to "build"
  when the build failed. Copy `results_file` globs into `results/`. Write
  `result.json {code, signal, build_s, run_s, phase}`. If a job is sent sharded, run the whole
  `command`. Shards across nodes are deferred (§9).
- **Under a step host.** Launch the command with `mason_step_host::launcher`, the same way
  `hostedcheck` does, so that a node daemon restart or takeover doesn't orphan the job. The job
  record `<job dir>/job.json` holds the state.
- **Limits.** `limit_s` is enforced on the node: kill at the limit and mark `timed_out`. The
  coordinator also stops waiting at the limit plus a margin and cancels.
- **Orphans.** If a job's lease (`lease_s`, default 300) runs out because the coordinator stopped
  polling, the node cancels it. This is the opposite side of coordinator restarts (§3.10).
- **Toolchain.** The node uses its installed toolchains. The coordinator only sends a job to a
  node whose `toolchains` cover `run_json.toolchains`, plus `xcode` when it is requested. This
  is about the same level of trust as a local run, which also honours `rust-toolchain.toml` from
  the tree. Write this down in the decision record.

**Trust.** A node's result counts the same as a GitHub runner's. The node is a machine the owner
paired. It runs only the command pinned by the coordinator's definition. Its output goes through
the same parser and the same minimum. No gate changes.

### 3.8 The coordinator executor

Write a new `node_exec.rs` that mirrors `remote::execute` (`remote.rs:1587`):

1. Write the record `<log_dir>/<id>.node {nid, job, sha, sent_ms}` **before** any network call,
   so that a restart finds the run.
2. Snapshot, then bundle negotiation, then `POST /node/jobs`. If the answer is `409` (the node
   filled up since its last status), try the next node. If no node has room, return
   `Fail::Environmental("no node had room")` so that the caller falls through to the GitHub or
   local path.
3. While the job runs, poll `GET /node/jobs/{id}` (which also renews the lease) and tail
   `/log?from=`. Feed the live output to the same live-count hook that `hostedcheck` uses, so
   progress shows up while the job runs, which GitHub runs can't do.
4. When it is done, fetch the artifact. Its results files are written only under plain relative
   paths (no `..`, nothing absolute); the artifact is JSON, so nothing is extracted (the plan first
   said a tar read with `read_artifact`'s allow-list). Build a `Finished` with `ran_on = "ran on node studio (arm64), picked up at
   once, 3 min"`, then `ack`.
5. If `stop()` asks to stop, send `DELETE` and return `Outcome::Stopped`.
6. If the node can't be reached for `[nodes] lost_after_polls` polls in a row (a count, not a
   duration, so tests can't depend on timing), give up with `Fail::Environmental`, best-effort
   `DELETE`, and remove the record. The caller places the run again.

`Finished` and `Outcome` don't change, so `remote_ran` (`suites.rs:723`) and everything after
it judge the run exactly as before.

### 3.9 Placement

Extend `Executor` and `Place` with a third variant:

```rust
pub enum Executor { Local, GitHub { runs_on: String }, Node { nid: String, name: String } }
pub enum Place    { Local {..}, GitHub {..}, Node { nid, name, from, why } }
```

Rework `decide()` (`place.rs:462`) into two separate questions:

- **May this run leave this machine?** It may not if the suite has `remote = false`, the
  environment leases a device, it uses `exclusive`, or the mode is `local`. These rules are the
  same as today.
- **Where off this machine?** Nodes no longer require a project `[remote_runs]` section, because
  that section is GitHub-specific. Use this order:
  1. A **node** with `accepting`, the toolchains, and room for the run's needs. Prefer a node
     with a warm mirror and a free slot for the project, then more free slots, then a cooler die.
     This needs `[nodes] enabled` and the project not opted out (`[nodes] projects` allow-list,
     or `exclude`).
  2. **GitHub**, under exactly today's rules (`[remote_runs]`, setup check, `balance`).
  3. Otherwise, stay **local**.

  `[nodes] prefer = "nodes" | "github"` (default `"nodes"`) can swap 1 and 2.

The modes keep their meaning, with "remote" now meaning "off this machine":

- `local`: here only.
- `remote`: node, else GitHub.
- `favor-local`: here until overloaded, then node, then GitHub.
- `favor-remote`: node, else GitHub; if both are full and this machine has room, here.

The `Facts` struct gets a `nodes: Vec<NodeRoom>` field filled from the status poller's cache.
`decide` stays **pure**. Add table-test rows for every new branch.

**Capacity on the coordinator.** Model each node as a broker resource `node:<nid>`. The status
poller sets its limit to the node's advertised free slots, plus whatever this coordinator
already runs there, so the limit means the node's real capacity. A node run takes one unit of it,
**in rank order**, through the normal broker, so step-first ranking still applies. It takes
**no** local `compile` or `test-suite` slot and **no** GitHub pool unit. Update `pool_needs`
(`remote.rs:2463`) and `goes_remote` (`hostedcheck.rs`) so that a node run, like a GitHub run,
doesn't wait on this Mac's CPU or thermal gates. The `HoldGuard` record gains a `node` field, so
a restarted coordinator counts the unit as held.

**Spill.** A run that is waiting for GitHub pickup moves to a node when one frees up. Use the
existing `spill_due` / `RoomFn` hook in `Site` (`remote.rs:294`), with a node `RoomFn` next to
the local one. This is phase 3.

### 3.10 Failures and restarts

| Event | Behaviour |
|---|---|
| Node switch turned off (default `drain`) | No new jobs. Running jobs finish and are collected normally. |
| Node switch turned off with `cancel`, or `mason node stop-jobs` | Running jobs are killed. The coordinator gets `cancelled_by_node` → `Fail::Environmental` → the run is placed again. No task is charged. |
| Node becomes hot or busy mid-job | The job continues, because there is no pre-emption. The node stops accepting new jobs. |
| Node unreachable (sleep, network, crash) | After `lost_after_polls` failed polls: `Fail::Environmental`, the run is placed again, and an alert appears ("node studio went away"). The node's lease then expires and it kills the orphan. |
| Node daemon restarts | Jobs keep running under step hosts. The node reloads `job.json` and polling resumes. |
| Coordinator restarts | On startup, adopt `<id>.node` records the way `reap_adopted` adopts GitHub holds. The run's caller re-attaches through the record when the step is resumed, or the job is cancelled if the step is gone. |
| Version mismatch | The node is listed as "incompatible (node_proto 1 vs 2)" and never used. |
| Bundle rejected or git error on the node | `Fail::Environmental` with the node's message, and the node is marked "unhealthy" for N minutes. |

### 3.11 CLI, views and the switch

- **Node machine:**
  - `mason node accept on|off`
  - `mason node status`: the same JSON as `/node/status`, run locally
  - `mason node pair-code`
  - `mason node jobs`
- **Coordinator:**
  - `mason node list`: discovered and paired nodes, with accepting, why_not, CPU, die °C, slots
    and jobs
  - `mason node add`
  - `mason node remove`
  - `mason node use on|off`: writes `[nodes] enabled` live, the way `remote.toml` is written
- **HTTP routes on loopback,** owner only, for the clients: `GET /nodes`,
  `POST /node/accept {on}`, and so on. Everything goes through one handler, as with every other
  route (`ws.rs` runs commands through the HTTP handlers).
- **Monitor (Swift):**
  - On a node, an **"Accept work from other Macs"** toggle on the home view and in Settings.
    This is the owner's working-hours switch, so it must take one click.
  - On a coordinator, a **Nodes** section, and the "GitHub" section (`remote_runs.rs`) becomes
    **"Remote runs"**, with node rows.
  - The task view's "ran on" line already shows `Finished.ran_on`.
  - `clients/features.toml` (client parity) is protected. Agree with the owner which clients
    must get the toggle.
- **Alerts:** "node studio went away", and "node studio incompatible". Use the existing alert
  plumbing, and `say` only, following the owner's notification choice. *Built:* `nodes::alerts`.
  *Hooked up* 2026-11-24 with the owner's approval: `attention::alerts_now` (a protected path §8
  does not list) calls `nodes::alerts(d)` beside `ssh_keys::alerts(d)`, one line.

### 3.12 Security of the work machine

This is remote code execution by design. The coordinator runs compiles and tests of agent-written
code on the Studio. The owner decided that **the node daemon runs as a separate macOS user**.
`docs/nodes.md` gives this as the only supported setup:

- A **separate standard macOS user** (for example `masonnode`, created by the owner) runs the node
  `masond` from a LaunchDaemon with `UserName=masonnode`. Jobs then can't read the owner's work
  files, Keychain or SSH keys. That user's home holds the toolchains (rustup, the Xcode
  selection, Java), the caches, the mirrors and the slots.
- Because the daemon runs under a LaunchDaemon, there is no GUI session for that user. Nothing
  can reach the owner's screen, and the `caffeinate` and `xcodebuild` shims still apply. The
  monitor app runs in the **owner's** session and talks to the node daemon to show the switch.
  So the node daemon's loopback API must accept the owner's local user. Check how the monitor
  finds and authenticates to a daemon of another user (socket permissions, port file location),
  and write down what `docs/nodes.md` needs for it. One way: a `mason node accept on|off` CLI
  that the owner runs with `sudo -u masonnode`, plus a group-readable port file.
- Bind the node listener to the LAN interface only (`[node] bind`, default `0.0.0.0`). The doc
  explains how to exclude VPN interfaces.
- The switch is off by default (§3.4).
- The node listener serves only `/node/*`, and only to paired coordinators' certificates (§3.3).

### 3.13 Landing: one queue per project, on its coordinator

The owner decided that landing is coordinated in one place, so that machines never conflict.

- **Each project has exactly one coordinator,** the `masond` it is registered on. That daemon
  owns the project's **only** landing queue. Nodes have no landing queues and no landing role
  for projects they don't own. A node that also has projects of its own (the design is
  symmetric) is that project's coordinator, and that project lands there.
- **The coordinator does every git step:**
  - forming batches by rank
  - merging into the integration worktree
  - **resolving conflicts** (the existing conflict handling and agent, unchanged)
  - the red ladder's bookkeeping
  - fast-forward promotion under the repo lock
  - the push

  Nodes never hold the project's push credentials and never get a ref they could push.
- **Only the test runs move.** A batch's suite run, its rerun of failures, and **each bisect
  half** are runs with no task. Today they follow the global switch (`executor_global`,
  `remote.rs:93`; "no task owns it", #702). They now go through the same placement as everything
  else, nodes first (§3.9). The node receives a snapshot of the **integration worktree**, which
  is the merged branch, exactly as GitHub does today. It runs the pinned suite and returns the
  output, and the coordinator judges it.
- **Bisect halves in parallel.** When several nodes have room, the two halves of a bisect step
  can run on different nodes at the same time, instead of one after the other. Look at how the
  red ladder sequences halves in `landing.rs` (the `red` closure near :5240). If running them in
  parallel needs more than a small change, record it as a follow-up and don't do it in this plan.
- **The "all suites go remote" check** at `landing.rs:5230`, which matches
  `Executor::GitHub { .. }`, must treat `Executor::Node` the same way ("off this machine").
  Review what that check is for before you change it. It decides whether landing can skip work
  that only matters locally.
- **No landing lock crosses machines,** because only one machine lands a project. Write down in
  the decision record that registering the same project on two coordinators is unsupported.
  `mason project add` should warn if a paired node reports that it has a project with the same
  remote URL (`/node/status` lists the node's own project remotes).

### 3.14 macOS privacy prompts and signing on a node with no user logged in

The node daemon runs as `masonnode` from a LaunchDaemon, while the owner is logged in as their
own work user. **Nobody can ever answer a privacy prompt for it.** When macOS wants a dialog for a
background process with no GUI session, the request is denied. The process then gets `EPERM`, or
in the worst case blocks. For a coordinator that looks like a stuck or red job for no visible
reason. The rule: **the node never needs a prompt.** Every permission it uses is granted ahead of
time by the owner, in the owner's own GUI session. Every grant is tied to a signature that does
not change across updates. The daemon checks its permissions itself and reports in words what is
missing. It never hangs.

**What MASON does today on the iMac (reuse it).**

- `update.rs` `sign_installed` / `codesign_args` (around :497–:530) signs every installed runtime
  binary with `sign_identity`, a Developer ID. That gives a team-anchored designated requirement
  and the identifier `com.brianm998.mason.<name>`, so privacy grants survive self-updates.
- `scripts/make-daemon-app.sh` wraps the launcher in **"MASON Daemon.app"**, signed the same way,
  so System Settings has an app to grant permissions to (Developer Tools, Full Disk Access).
- masond is the *responsible process* for everything it starts. Its grants cover the cargo,
  rustc and test processes under it. The update.rs comment about `/Volumes/sp` and tccd explains
  why an unsigned or ad-hoc masond is dangerous.

**Problem 1: the node can't sign its own updates.** The Developer ID private key is in the
owner's login keychain on the iMac. `masonnode` has no access to it, and `codesign` would hang
waiting for keychain approval until `SIGN_TIMEOUT`. An ad-hoc signed node changes its designated
requirement on every build, which drops every grant, and nobody is there to approve them again.

**Fix: the coordinator delivers the node's runtime, already signed.** This fits "everything comes
from the coordinator".

- When the coordinator's `mason update` builds a new runtime and there are paired nodes of
  another architecture (the Studio is `arm64`, the iMac `x86_64`), it **also builds for that
  target** (`rustup target add aarch64-apple-darwin`, then `cargo build --target …`). It signs
  the result with the same `sign_identity` and packs it with MASON Daemon.app.
- It sends the package with `PUT /node/runtime`. The node verifies it with
  `codesign --verify --strict` and a requirement pinned at pairing:
  `anchor apple generic and certificate leaf[subject.OU] = "<team id>"`. The team id is taken
  from the coordinator's signed binary at pairing and stored next to the fingerprint in
  `coordinators.json`. The node then installs it under `runtimes/` and takes over with the
  existing exec takeover. **A node never builds or signs MASON itself.** This also keeps node and
  coordinator versions in step, as §3.1 requires.
- **First install:** `mason node export-runtime --arch arm64 --out <dir>` on the coordinator
  writes the same signed package. The owner copies it to the Studio and installs it from
  `docs/nodes.md`.
- The cross build costs the iMac one extra build per update until January. The rejected option
  is to put a copy of the Developer ID identity in a dedicated keychain for `masonnode`. That is
  a signing secret on the work machine, so don't do it unless the owner asks.

**Problem 2: volumes.** The owner noted that the node needs a volume other than its boot
volume. macOS guards removable volumes (USB or Thunderbolt disks) and network volumes (SMB, NFS)
separately, with per-user prompts. Those prompts can never be answered here.

- **Preferred: keep everything the node writes on the Studio's internal disk.** That is
  `--data-dir` under `/Users/masonnode`, with mirrors, slots, `~/.cache/mason-target`, cargo and
  sccache caches. No volume permission is needed, there are no USB stalls (CLAUDE.md "USB disk
  stall"), and it is fastest.
- **If a volume really is needed,** grant **Full Disk Access to MASON Daemon.app** (System
  Settings → Privacy & Security → Full Disk Access, added by the owner with admin
  authentication). It is stored in the system-wide privacy database. It applies to that signed
  app whichever user runs it, and it covers removable and network volumes for masond and every
  process it starts.
  - The node checks this at start (below). Check on the real Studio that a LaunchDaemon running
    as `masonnode` really is covered. If it isn't, record what was needed.
- **External disk:** macOS mounts external disks only after someone logs in, unless the owner
  sets `AutomountDisksWithoutUserLogin`. The command goes in `docs/nodes.md`:
  `sudo defaults write /Library/Preferences/SystemConfiguration/autodiskmount
  AutomountDisksWithoutUserLogin -bool true`. Also, `masonnode` must be able to write to the
  disk: either "Ignore ownership on this volume", or a directory owned by `masonnode`.
- **Network share:** a share the owner mounts in their own session belongs to the owner and
  vanishes when they log out, so the node can't rely on it. It would need a system-level mount
  (autofs, `/etc/auto_master`) that holds the share's credentials on the Studio. Build
  directories on SMB are also slow and unreliable for cargo's file locking. **Don't put slots or
  target directories on a network volume.** If the owner really needs one for inputs, it goes
  into `docs/nodes.md` as an autofs setup and gets its own decision record.
- **Work-machine caveat:** if the Studio is managed by an employer's MDM, a profile may block or
  pre-set Full Disk Access. `docs/nodes.md` tells the owner to check System Settings → Privacy &
  Security → Profiles first.

**Problem 3: other privacy grants.**

- **Developer Tools** (it skips the malware scan on every launch of freshly built code; see
  CLAUDE.md "syspolicyd down"): add MASON Daemon.app on the Studio as on the iMac. If this grant
  turns out to be stored per user and not system-wide, the owner logs in as `masonnode` **once**
  (fast user switching), adds it in System Settings there, and logs out. That is a one-time step
  in `docs/nodes.md`.
- **Local Network** (macOS 15+). On the node, the daemon listens on the LAN and advertises with
  mDNS. Apple's documentation says launchd *daemons* are not subject to local network privacy.
  Check that on the Studio with a LaunchDaemon running as `masonnode`. If a prompt would appear,
  record the fix. On the **coordinator** (the iMac, a LaunchAgent in the owner's session), the
  first connection to a node raises "Allow MASON to find devices on local networks?". The owner
  can answer it there, and the Team ID signature keeps the answer across updates. **Check
  first:** on 2026-11-22 a scratch exit-script daemon raised this dialog, charged to the same
  bundle `com.brianm998.mason.masond`, and the advice was "Don't Allow". If the owner clicked it,
  the production coordinator is denied too. Turn it on in System Settings → Privacy & Security →
  Local Network before Phase 1's manual check. `mason node list` must say "local network access
  denied by macOS" in that case, not just "unreachable".
- **Nothing else.** No Accessibility, Screen Recording, Input Monitoring, Automation or camera.
  The node runs no UI tests (§3.7), so it never needs them.

**`mason node doctor`, and the start-up probe.** The node runs the same checks on start, every 10
minutes, and on demand with `mason node doctor`. The results go into `/node/status` as
`accepting=false` with a `why_not` the owner can act on:

- **Signature:** its own executable and app have a team-anchored designated requirement
  (`codesign -dr -`). If the signature is ad hoc: "masond is signed ad hoc; privacy grants will
  not hold — install the runtime from the coordinator".
- **Paths:** for every path it will use (data dir, mirrors, slots, cache and target dirs, any
  volume the owner configured), create, write, read and delete a probe file. Do this on a thread
  with a deadline, so a stalled disk or a privacy denial reports instead of hanging (CLAUDE.md
  "USB disk stall"). `EPERM` on a volume reads: "macOS denied access to /Volumes/X: give MASON
  Daemon.app Full Disk Access".
- **Volume mounted:** a configured volume is mounted ("/Volumes/X is not mounted; see
  AutomountDisksWithoutUserLogin in docs/nodes.md").
- **Local network:** the listener is bound and the mDNS registration succeeded.
- **Toolchains:** `cargo --version`, `xcode-select -p` and `java -version` run **as masonnode**
  without prompting. A first `xcodebuild` as a new user may need `sudo xcodebuild -license
  accept` and `sudo xcodebuild -runFirstLaunch`; put both in `docs/nodes.md`.

---

## 4. Configuration reference

Coordinator, in `daemon.toml`:

```toml
[nodes]
enabled = true            # use paired nodes (live: `mason node use on|off`)
prefer = "nodes"          # or "github": which off-machine target is tried first
static = []               # ["studio.local:7425"] when mDNS can't see the node
status_every_s = 5
lost_after_polls = 6
# projects = ["mason"]    # optional allow-list; default: every project
# exclude  = []

# Settings sent to a node with every status poll (§3.4). [nodes.default] applies to every
# node; [nodes.<name>] overrides it for one. Read live, so no restart is needed.
[nodes.default]
max_jobs = 4
max_other_cpu_pct = 50
min_disk_free_gb = 50
on_switch_off = "drain"   # or "cancel"
lease_s = 300
max_die_c = 0             # 0: use the node's thermal defaults
# idle_min = 0            # phase 5

[nodes.studio]
max_jobs = 6
```

Node: **this is the whole of its configuration** in `daemon.toml`:

```toml
[node]
serve = true
# port = 7425             # optional
# bind = ["0.0.0.0"]      # optional
```

A node also has two state files that nobody edits by hand:

- `<config>/node.toml`: the switch (`accept`, `changed_by`, `changed_at`; `accept = false` by
  default), changed with `mason node accept on|off` or the monitor, and read live.
- `<data>/node/coordinators.json`: the fingerprints of the paired coordinators, written by
  pairing.

It has no tokens, no project registrations and no tunables.

---

## 5. Phases

Each phase ends in a working state with the feature **off unless configured**. Pushes to develop
happen only at the points in §10, item 4b. Every phase: SPDX headers, `git commit -s`, `cargo fmt`,
`cargo clippy --workspace --all-targets -- -D warnings`, the narrowest tests that cover the change,
and the exit script for the phase.

### Phase 0: make room for a third executor (no behaviour change)

- Add `Executor::Node` and `Place::Node`, which are not produced yet. Add the `Node` arm at every
  match site: `suites.rs:457`, `hostedcheck.rs:659/678`, `landing.rs:5230`, `covmap.rs:523`, and
  `Place::text/from/is_github` (rename `is_github` to `is_off_machine`, or add it alongside).
- Split `decide()` into "may leave" and "where off-machine" as in §3.9, with GitHub as the only
  off-machine target for now. The existing table tests must pass unchanged. Add rows for the
  split.
- **Exit:** all existing place, remote and suites tests pass, and decision output is identical.

### Phase 1: identity, pairing, discovery, status (no jobs)

- `config.rs`: add `NodeCfg` (`[node]`) and `NodesCfg` (`[nodes]`) with parsers like `parse_lan`.
  Bad values are errors that name the setting.
- `node_serve.rs`, the listener:
  - the node listener with a **required client certificate** (§3.3); `access.rs` and the phone
    LAN guard are left alone
  - the per-connection fingerprint extension and the guard checking `coordinators.json`
  - `/node-pair`, using `Pairing` for the codes
  - the mDNS advertisement, with the TXT keys from §3.2
- `node_serve.rs`, status and switch:
  - the switch file (`node.toml` and its log)
  - `POST /node/status`, keeping the coordinator's settings in memory and applying the strictest
    value across coordinators
  - the availability decision as a **pure function** of a readings struct and the settings, with
    table tests: switch off, draining, hot, other CPU high, no slot, disk low, all clear
  - `other_cpu_pct`
- `nodes.rs`:
  - the mDNS browse thread and static hosts
  - the `nodes` store table (a migration in `crates/store`)
  - `[nodes.default]` and `[nodes.<name>]` settings, read live
  - the status poller (one thread, every `status_every_s`), which updates a cache that `Facts`
    reads
- `node_client.rs`: hyper + rustls, presenting the coordinator's identity
  (`load_or_create_identity`) as the client cert and pinning the node's fingerprint. Unit tests:
  - a wrong server fingerprint is refused
  - an unpaired client cert gets 403
  - a paired one is accepted

  Run them against a local test listener.
- CLI:
  - on the node: `mason node pair-code | accept on|off | stop-jobs | status | coordinators [remove]`
  - on the coordinator: `mason node add | remove | list | use`
- **Signed runtime for the node (§3.14):** `mason node export-runtime --arch <arch> --out <dir>`
  (a cross build plus `sign_installed` with `sign_identity`, packaged with MASON Daemon.app).
  On the node, a verified install (`codesign --verify --strict` against the team id pinned at
  pairing). Tests:
  - the requirement check refuses an ad-hoc package and a package from another team; use a fake
    `codesign` runner, as `update.rs` tests do
  - `codesign_args` output for the cross-built binaries
- **`mason node doctor` and the start-up probe (§3.14):** signature, paths with a deadline,
  volumes mounted, listener and mDNS, toolchains. Each failure becomes `accepting=false` with an
  actionable `why_not`. Table tests run over injected probe results.
- **`docs/nodes.md`, first version.** The owner sets up the Studio from this doc, so write it in
  this phase:
  - creating the `masonnode` standard user
  - installing toolchains in that account (rustup with the pinned Rust, the Xcode selection,
    `sudo xcodebuild -license accept` and `-runFirstLaunch`, Java)
  - installing the **coordinator-exported signed runtime** and "MASON Daemon.app", plus a
    LaunchDaemon plist with `UserName=masonnode` whose program is inside the app
  - privacy grants in the owner's session: **Full Disk Access** and **Developer Tools** for MASON
    Daemon.app. If Developer Tools turns out to be per-user, include the one-time GUI login as
    `masonnode`
  - checking for an MDM profile
  - keeping node data on the internal disk; if a volume is used, `AutomountDisksWithoutUserLogin`
    and ownership
  - on the **iMac**: Local Network enabled for MASON in System Settings (§3.14)
  - running `mason node doctor` until it is clean
  - `[node] serve = true`
  - opening the firewall for the node port
  - pairing from the iMac
  - turning the switch on and off
  - checking with `mason node list`
  - troubleshooting: mDNS not seen, use `static`; fingerprint mismatch; syspolicyd hangs on
    first launch

  Phase 4 adds the monitor toggle to it.
- Apple Silicon thermal check on the Studio (§3.4 point 2).
- **Exit test:** two daemons on loopback with separate data and config dirs. B has
  `serve = true`; A has `static = ["127.0.0.1:<B port>"]`. Pair with a code; `mason node list`
  on A shows B as `accepting=false (switch off)`; `mason node accept on` on B; A shows
  `accepting=true`; A's settings (`max_jobs = 3`) show in B's `mason node status`; remove A from
  B's coordinators; A shows "not paired (403)". B's data and config directories contain no token
  and no copy of A's settings on disk. No mDNS in the automated test, because
  multicast is unreliable in CI. mDNS gets a TXT-parsing unit test and a manual check.
- **Manual:** on the real Studio, `mason node list` on the iMac shows it through mDNS with its
  real CPU and temperature. `mason node doctor` on the Studio is clean while **nobody is logged
  in as masonnode**, and with the owner logged in as their work user. Record which grants were
  really needed (Full Disk Access, Developer Tools, Local Network) and fix `docs/nodes.md` to
  match.

### Phase 2: run jobs on a node

- `node_serve.rs`: mirrors, the bundle endpoints, slots, the job endpoints, the lease and the GC.
- `node_run.rs`: the runner (§3.7). Write the environment audit table in the decision record.
  Apply the shims.
- `node_exec.rs`: the executor (§3.8), with the record file and resume.
- Placement: fill `Facts.nodes` from the poller, return `Place::Node` (§3.9), and add the broker
  resource `node:<nid>`, `pool_needs`, `goes_remote` and `HoldGuard`.
- Wire it in:
  - suite runs: in `suites.rs`, next to `run_remote`, add a `run_on_node` returning the same
    `Ran`
  - `run_check`: `hostedcheck.rs`, sending `build` and the command as one job as with #771
  - landing (§3.13): the batch suite run, the rerun of failures and the bisect halves go through
    placement with no task, nodes first; the check at `landing.rs:5230` treats `Node` as off this
    machine. Merging, conflicts, promotion and the push stay on the coordinator.
- **Exit test:** two loopback daemons. A has a tiny project with a suite (fixture), and the switch
  on B is on. A task's test step runs on B. A's verdict, its output and its results file match a
  local run of the same suite. `ran_on` names the node. Then a red suite gives the same red
  verdict. Any binaries the fixture copies are signed with `codesign -s -`.
- **Exit test (landing):** A lands a batch of two fixture tasks. The batch's suite runs on B, and
  A promotes and pushes to a local bare "remote". B's directories contain no push configuration.
- **Manual:** set mason's global mode to `remote` on the iMac with `prefer = "nodes"`, and watch
  real test steps and landing batches run on the Studio. Compare wall time with the GitHub runs of the same suite.

### Phase 3: robustness

- Switch off with `cancel` and `drain`. Node lost, so the run is placed again. Coordinator restart
  adoption. Orphan lease on the node. A `412` resend. Incompatible versions. The spill from the
  GitHub queue to a node.
- Live log tail and live counts from node runs.
- **Runtime updates pushed to nodes (§3.14):** after the coordinator's `mason update` installs a
  new runtime, it cross-builds and signs for each paired node's architecture and sends
  `PUT /node/runtime`. The node verifies, installs and takes over after its running jobs drain.
  A node on an older `node_proto` stays "incompatible" until it has been updated.
- Alerts.
- **Tests:** each row of §3.10 as an integration test on loopback, using counts and conditions,
  **never durations**. Use an injectable clock or poll counter where the code waits.
- **Exit tests:**
  - (drain, the default) a job is running on B; `mason node accept off` on B; B takes no new job,
    the running one finishes on B, and A collects it green
  - (cancel) the same with `on_switch_off = "cancel"`, or with `mason node stop-jobs`: the run
    finishes on A (local fallback) with a green verdict, and B's job directory is cleaned up

### Phase 4: views and the monitor

- `GET /nodes` and the owner routes. `remote_runs.rs` becomes "Remote runs" with node rows.
- The Swift monitor: the accept toggle (node), the Nodes section (coordinator), and the "ran on"
  text. Swift screenshots are rendered **offscreen** only (ImageRenderer).
- Settle the `clients/features.toml` parity entries with the owner.
- `docs/nodes.md`: add the monitor toggle, and how the owner's session reaches the node daemon of
  the `masonnode` user (§3.12).

### Phase 5 (optional, separate plan when wanted)

Idle detection, a working-hours schedule, simulators and devices on nodes (the Studio could run
the Preffect and timelapse simulator suites), shards across nodes, agent sessions on nodes,
Linux nodes. (Runtime updates pushed to nodes are now in Phase 3, §3.14.)

---

## 6. Files

New files:

- `crates/daemon/src/nodes.rs`: the coordinator registry, browse and poller
- `crates/daemon/src/node_client.rs`
- `crates/daemon/src/node_exec.rs`
- `crates/daemon/src/node_serve.rs`
- `crates/daemon/src/node_run.rs`
- the CLI subcommand file
- the store migration
- `docs/nodes.md`
- a decision record
- one exit script per phase

Changed files:

- `remote.rs`: the `Executor`, `pool_needs` and `HoldGuard` changes
- `place.rs`
- `suites.rs` *(protected)*
- `hostedcheck.rs`
- `landing.rs` *(protected)*: placing the batch, rerun and bisect runs (§3.13)
- `covmap.rs`
- `lan.rs`: `advertise`, made reusable by the node listener
- `config.rs`
- ~~`sched.rs`: dynamic limits for `node:<nid>`~~ — `sched.rs` is protected and §8 does not approve
  it. The poller sets `node:<nid>`'s limit through the public `Broker::set_limit` instead, so
  `sched.rs` is not edited.
- `thermal.rs`
- `remote_runs.rs`
- `http.rs`
- `crates/daemon/Cargo.toml`: hyper client features
- the Swift monitor

---

## 7. Rules for the executing agent

- Work in a worktree under `.claude/worktrees/<branch>` on branch `claude/mason-nodes`, cut from
  `origin/develop`. Read code from `origin/develop` (see CLAUDE.md).
- Follow the repository footer rules: SPDX lines, `git commit -s`, fmt, clippy, tests, exit
  scripts, no weakening of gates, and no edits to protected paths beyond those the owner approved
  in §8.
- **Reserve numbers** for every exit script, stage note and decision record before you write it
  (`number_reservations`). Check open owner questions too, because an answer may already have
  used a number.
- **Interactive `cargo` jobs take compile slots ahead of the pipeline,** and the iMac is failing.
  Keep builds few and run narrow tests (`cargo test -p mason-daemon <module>::`). Once Phase 2
  works, consider doing the remaining development on the Studio itself.
- **No timing assertions.** Fixtures that copy binaries ad-hoc sign them. Nothing appears on any
  screen. Stop test daemons by the pid in their `daemon.json`, never with `pgrep -x masond`.
- Merge `origin/develop` into the branch at least daily. Push to develop only at the three points
  in §10, item 4b. Before pushing: rebase, rerun the tests you touched, and check that the
  upstream log is green.
- Never write the other pipeline's name in commits, docs or code.
- Don't restart the owner's running `masond` to test. Use your own test daemons. The owner takes
  a build over with `mason update`.

---

## 8. Protected paths this plan touches (approved by the owner, 2026-11-23)

| Path | Why | Phase |
|---|---|---|
| `crates/daemon/src/suites.rs` | Add the `Executor::Node` arm and `run_on_node`, returning the same `Ran` that `run_remote` returns. The verdict code is unchanged. | 0, 2 |
| `crates/daemon/src/landing.rs` | The `Executor::Node` arm at :5230, and sending batch, rerun and bisect test runs through placement (§3.13). Merging and promotion are unchanged. | 0, 2 |
| `scripts/exit/sNNN.sh` (new) | One exit script per phase. | 1-3 |
| `crates/daemon/src/update.rs` | Reuse `sign_installed` and `codesign_args` for the cross-built node runtime, and hook "send runtime to nodes" after a takeover (§3.14). The rollback and no-downgrade rules stay as they are. **Added after the first approval; the owner approved it separately on 2026-11-23: approved (2026-11-23).** | 1, 3 |
| `deny.toml` | Only if the hyper client features pull in a crate that isn't allowed yet. | 1 |
| `clients/features.toml` | The parity entries for the accept toggle and the Nodes section. | 4 |
| `crates/daemon/tests/fixtures/**` | Only if a golden event log changes because of new events (node lost, and so on). | 3 |

---

## 9. Not in this plan

Agent (LLM) sessions on nodes. They are API-bound, and their heavy part, `run_check`, already
moves to a node through this plan. Also not included: simulators and devices on nodes, shards
across nodes or across a node and GitHub, working-hours schedules, idle detection (phase 5),
Linux nodes, and NAT or internet nodes (Tailscale would work with
`static`, but it isn't designed for here).

---

## 10. The owner's answers (2026-11-23)

1. **Switch-off:** running jobs **finish** (`on_switch_off = "drain"` is the default). Cancelling
   is available per node, and as `mason node stop-jobs`.
2. **Separate macOS user** for the node daemon: **yes**. It is the only supported setup (§3.12).
3. **Nodes before GitHub:** **yes** (`[nodes] prefer = "nodes"`).
4. **Landing:** **one landing queue per project, on the coordinator,** which does every git step
   including conflict resolution. It may test the merged branch locally or on a node (§3.13).
   - **4b, how this plan's own commits reach develop:** one branch, `claude/mason-nodes`, with
     `origin/develop` merged into it **at least daily**. This work touches the files that change
     most often on develop (`place.rs`, `remote.rs`, `suites.rs`, `hostedcheck.rs`,
     `landing.rs`), so a long-lived branch would otherwise end in heavy conflicts. Push to
     develop only at these points:
     1. **Phase 0**, once it is green, because it changes no behaviour and touches every call
        site.
     2. **When the feature is usable,** after Phase 3, or after Phase 2 if the owner wants to
        start using it sooner. Ask the owner which.
     3. **Phase 4** (the monitor and docs), once it is done.

     Every push has the feature off unless configured. Before each push: rebase, rerun the tests
     you touched, and check that the upstream log is green.
5. **The protected paths in §8:** approved.
6. **No auth configuration or token storage on nodes.** The coordinator delegates trust: mutual
   TLS with the coordinator's certificate, pinned by the node at pairing, and node settings sent
   by the coordinator (§3.3, §3.4, §4).
7. **Studio setup:** the owner does it from `docs/nodes.md`, which Phase 1 delivers.
