<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1222: one catalogue of per-project settings, and which keys a paired device may not write

Task #1243. Stage s1232.

## Decision

1. **A catalogue in the daemon.** `projsettings::KEYS` describes every per-project registration-file key: section, type and choices, default, sensitivity, and when a change applies. `GET /projects/{name}/settings` returns it under `catalogue` (the older fields stay, so older monitors keep working); `POST` on the same route writes any catalogued key. Clients draw the page from it. A file key the catalogue does not know is shown as unrecognised and read-only.
2. **Writing keeps decision 0423**: `toml_edit`, the loading parsers, an atomic write, the reload, and the old text back if the reload refuses. Every writable key can be unset. Keys fixed at start (`FIXED_KEYS`) are shown and refused with "restart the daemon".
3. **Secrets** are never returned or accepted: a reference (`env:NAME`, `file:/path`) or "set in file, not shown". Names in `[agent_env]` and `test_env` that look secret are hidden and cannot be written from the page.
4. **Keys that run a program or weaken a guard are written only from a monitor on the daemon's own Mac**, with confirmation and the diff. They are `test`, `build`, step `command`/`build`, `claude_bin`, `codex_bin`, `[agent_env]`, `sandbox`, `[agent] user_skills`, `clarity_classifier`, `card_rewriter`, `[triage] command`, `[ci]` commands and URLs, schedule `command`, and every `api` (a changed API address sends the token elsewhere). A request through a paired device, an owner's included, is refused them. The daemon knows a request is paired from `x-mason-paired`, set by the LAN guard and the command relay and never taken from the client.
5. **When it applies**: a reload makes the file the current registration, and `jobs::ctx` reads it when each step run starts, so the next step run of every task uses it. The old wording ("tasks admitted from now on") was wrong for the registration file; it is right for pipeline definitions.

## Why

The speech command was left out of the Settings page (0423) because writing a command from a remote client is remote code execution on the owner's Mac. The Project page would reintroduce that for `test`, `build` and the rest unless the daemon, not the client, refuses them from paired devices. `access::decide` sees no body and allows an owner device everything, so the check is in the route.

## Needs the owner's approval

Item 4 widens what a monitor can write (program keys from the Mac itself, with confirmation) and narrows what a paired owner device can do on this route. No protected path was changed.
