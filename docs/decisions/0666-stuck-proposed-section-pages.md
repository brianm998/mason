# 0666: the stuck and proposed tiles get section pages

**Decision.** Add `needs_attention` and `proposed` to `Page.sectionNames` in both clients, rather than special-casing the tile handlers.

**Why.** `tileDoubleClicked` and `tileTarget` already open `Page.sectionPage(state)`; those two states had no page, so only the sidebar moved. The generic stage rows page lists the sidebar's rows, and Approve on the proposed page repeats the sidebar's rule.
