# 0677: impact override entries

The impact map was a pure union, so a narrower `= []` under a broader glob did nothing (star's
`scripts/exit/**`). Decision: a separate opt-in table form `{ suites = [...], override = true }`;
plain lists keep the union meaning, so a definition without overrides selects exactly as before.

Specificity is the number of wildcard-free glob segments, then the literal-prefix length; equal
overrides union. Plain entries at least as specific as the winning override still add, so a
targeted file entry can sit under an override. The data model keeps `entries` and adds `overrides`
instead of changing the entry type, so existing readers are untouched.

The dead-entry warning is advisory (never refuses) and uses a sample path of the empty glob.
