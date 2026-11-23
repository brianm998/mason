# 0991: impact-map bound count

## Context

`masons_own_impact_map_selects_suites_by_path` failed at 5b60d4554. `[binding.unit]` binds every unit-level suite to development and the test step, which is now eight. `guards` and `exit-guards` are unit-level but are not in `[step.test] suites`, which has six.

## Decision

The test follows the definition: `all.len() == 6`, bound counts 8. The step list is unchanged.

## Trust

`.mason/mason.toml` is protected. Guards and exit-guards run under run_check but not in the test step's list. Adding them is the owner's decision.
