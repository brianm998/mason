# 0551: check progress lives in the develop run's progress record

Status: accepted (s558).

Context: a develop step's `run_check` showed only "Running check <name>"; waiting for a slot, building and running looked the same.

Decision: each call is an entry in `Progress.checks`, counts are a `check <suite>` sub-count fed by `LiveCount` from `hostedcheck::run_with`, and the daemon words the lines (`mason_core::checks`) so clients can show them as they are. `rollup` skips agent steps. Rejected: a second record or store table (surviving a restart is not worth a schema change here); parsing in `suites.rs` (a protected verdict file, left untouched).

Trust: no protected path is touched (`suites.rs` and `clients/features.toml` unchanged).
