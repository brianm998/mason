# 0390: a retry diagnosis is applied by itself when the code was not at fault

**Decision.** A `retry` verdict is applied without the owner for any project when the failure class is outside the code, and for a code failure when the diagnosis sets `no_code_change` (MASON's own project keeps applying every retry). At most one per task per day; if the retry loop already has a retry pending (`autoretry::pending`) the diagnosis stays a card, so one failure is not retried twice. `explore`, `close` and `ask` stay cards.

**Why.** 20 diagnoses waited for the owner on 2026-11-10 to 11-14, most saying "run it again, no code change". A retry from a step is cheap and reversible, and it is guarded by the same `seen` check as any retry.

**Trust.** `diagnose.rs` is not protected. This widens what runs unattended, so it is limited to retries that need no code change.
