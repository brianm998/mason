# 1128: the thermal gate carries which condition holds

## Context

The thermal gate has three conditions (speed limit, die temperature, SMC power limiting) but the scheduler's gate could only express a speed against a minimum. The daemon encoded the other two as a minimum one above the speed, which produced sentences with an unreachable threshold ("work starts at 101% or more") and pointed the owner at the wrong gate.

## Decision

`ThermalGate` carries the condition as a `hold` field, and `min_pct` always means the configured `thermal_min_pct`. The wait cause carries the same field, and one function in the scheduler words it, so the waiting line, the stored event and the home page cannot disagree.

- A die hold names `thermal_max_c` as the release temperature: work is held at or over it and starts below it. There is no temperature hysteresis; the reading is the hottest of the last 60 s.
- An owner's update build is held only by the hard ceiling (`thermal_max_c + 2`), so its wait names the ceiling.

## Rejected

Keeping the encoding and having each message ask the daemon for the real reason: the scheduler's own sentences (the waiting line and the stored event) have no way to ask.
