# 0856: the init and adopt logic is a library crate, `mason-onboard`

Status: accepted (task #884; no PROTECTED path changed).

The monitors will add projects through the daemon, but the guided setup lived in the `mason-pipeline` CLI binary, which depends on mason-daemon. Decision: move it to `crates/onboard`, which depends on neither. Terminal I/O, registration with masond and the proof task stay in the CLI. The daemon's overlay parser cannot be called from the crate, so the overlay check is a caller-supplied `Validator`; the CLI passes the daemon's `parse_project`, keeping its behaviour. `plan` and `apply` take the CLI's flag set (`Flags`) so the CLI and the monitors accept the same answers.
