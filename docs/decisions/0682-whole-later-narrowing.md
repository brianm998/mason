# 0682: `whole_later` lets the test step narrow past its whole-suite fallbacks

Decision: the three fallbacks of `narrow::select` (a `narrow_never` hit, an empty pick, a pick over
half) stay as they are. A suite that declares `whole_later = "batch" | "ci"` uses
`narrow::select_later` instead, which never returns "whole" for them. The landing tier or the base's
CI proves the union, so the step proves the change.

Why validation sits in the landing config reader: landing tiers live in `.mason/landing.toml`,
which the definition parser does not see. A claim that fails is refused when the config is read,
and the step ignores it (runs as before) if it does not hold.

Trust: `crates/testing/**` and `suites.rs` are protected; a suite only narrows harder when its
definition says it runs whole later, and the `batch` claim is checked.
