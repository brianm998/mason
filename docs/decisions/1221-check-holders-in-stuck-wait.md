<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1221: check holders give stuck:wait no run-length bound

**Decision.** A waiter whose holders include a `#N check` lease has no position x run bound.
`stuck:wait` fires for it only when no grant came for `wait_ms`; otherwise `stuck:aged-wait`.

**Why.** The holder's run is one job, not the check, which runs for hours.

**Rejected.** The lease's expected duration: holders do not carry it in the ledger. A "granted
within minutes" rule: it would change behaviour for non-check holders too.
