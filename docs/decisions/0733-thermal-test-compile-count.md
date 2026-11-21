# 0733: Thermal test config, not the gate, was at fault

The test starts five compiles but its config left `compile_jobs` at the default of 3, so cc4 stayed waiting after the power limit lifted (#752).

**Chosen.** The test sets `compile_jobs = 8`. No assertion changed and no production code changed.

**Not chosen.** Changing the default or the thermal gate; both behave as designed.
