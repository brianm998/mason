# 0575: scratch worlds are capped machine-wide; git is resolved once

Status: accepted (s582).

Context: the per-binary world limit let N test binaries run 8·N daemons, and every git call went through the xcrun shim; the Mac hung four times on 2026-11-17.

Decision: slots are flock lock files in `/tmp/mason-test-worlds` (not `$TMPDIR`, which a task may set apart), and git is resolved once per process (`MASON_GIT`, `xcrun -f git`, `git`). Rejected: a lock server (more moving parts), a per-suite limit (does not cap across tasks).

Trust: no protected path is touched; `.mason/**` is unchanged.
