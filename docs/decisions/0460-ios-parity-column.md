# 0460: iOS is a third parity column, with differences as data

Status: accepted (owner approval needed: gate and client-parity files)

The gate compared only swift and compose. The iOS app shares the Swift views, so it gets its own `ios` column. A single value cannot say "iPad has it, iPhone does not", so a feature that differs sets `ios = "difference"` and names, per surface, `done` or an entry in `[ipad.*]` / `[phone.*]`; each entry must carry a `platform_reason`. Unknown names, missing reasons and a bare `todo` or `n/a` fail. Android is left out (not in scope; Compose's phone side is not built).
