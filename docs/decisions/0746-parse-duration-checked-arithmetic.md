# 0746: parse_duration uses checked arithmetic

Status: accepted (#748)

An overflowing `window` value (e.g. `18446744073709551615h`) must be an invalid config value, not a panic or a silently wrapped duration. `parse_duration` now returns `None` on overflow, so `parse` reports the usual "window" problem. Behaviour for all other inputs is unchanged.

Not verified: nothing beyond the unit tests.
