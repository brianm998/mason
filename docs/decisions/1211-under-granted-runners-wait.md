<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1211: an under-granted run waits, it does not fail

**Decision.** A run granted fewer runner units than `min_shards` waits on GitHub for a grant of
at least that many when it has no local fallback, as it already did when this Mac was barred.

**Why.** The refusal read as a failed check, so agents re-ran the starved run and spent
attempts on a shortage that is not about the code.

**Kept.** `fallback = "local"` still runs here. A pool that can never grant `min_shards` still
gives the environmental error.
