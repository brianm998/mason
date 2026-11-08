# 0043: Readings made while giving every adapter one usage shape (s62)

- **`blocked` is derived from window state and overage together.** A window is `blocked` when its status is `rejected` or anything not known (when unsure, blocked); `allowed_warning` is `warning`, which blocks nothing.
  - Any blocked window, with overage not in use: `blocked`, by the window that clears last, until its `resetsAt`.
  - A blocked window while overage is `InUse`: not blocked. Extra billing is running; the snapshot shows `InUse`, so "extra billing" and "blocked" are told apart.
  - A blocked window with overage `UsedUp`: blocked, `OverageUsedUp`, until the overage reset if given, else the window's.
  - A top-level `rejected` with no window to name: blocked, `Unreported`.
  - Only the `rejected`/`org_level_disabled` overage pair appears in the real captures. `isUsingOverage` with `overageStatus` allowed, and `overageResetsAt`, are assumptions in the synthetic fixtures. If real streams differ, this rule is where to change it.
- **Overage mapping:** using overage and status `rejected` is `UsedUp`; using overage otherwise is `InUse`; `rejected` while not using it is `Disabled(reason)`, or `Off` with no reason; no overage field at all is `NotOffered`; the rest is `Off`.
- **Billing comes from the stream.** The init event's `apiKeySource` is `none` on a plan; any other value is read as an API key. Spend is `total_cost_usd` only then; on a plan that figure is notional and is not offered as spend. The adapter is not told the billing kind at launch, so nothing is guessed from the environment.
- **A 429 with no window is `RateLimit`** (clears in minutes), never a plan limit. This also applies to an API account, which has no windows.
- **Windows:** ids come from `unifiedWindows`; a window named only at the top level (`rateLimitType`) is added from the top-level status and reset, with no utilization. An id not known yet passes through, labelled with the id, with no length. A utilization outside 0..=1 is dropped, not clamped.
- **Codex declares no windows.** No Codex CLI was installed, so the failure-text path is the only source; it gives `blocked: Unreported(text)` with no reset and no window.
- **The hold is unchanged.** Outcome classification (`UsageLimit`, `ProviderDown`, verdicts) reads the new snapshot but means what it meant: any rejected window, a 429, or limit text. A rejected window with overage in use still makes a failed result a hold.
- **Gate change (conformance):** the kit now checks usage. A model promising windows must emit all of them in the usage-limit scenario; utilization must never be emitted unless the model reports it. This only adds checks; nothing was loosened. The Claude harness's usage-limit scenario moved to a fixture that carries all three windows.
