# 0757: repetition exit scripts read MASON_EXIT_REPEAT

Decision: a script that repeats a cargo test must read `MASON_EXIT_REPEAT` (default 10), so the push run (2) stays under the per-script limit while the nightly run keeps 10. Rejected: raising the 900 s limit (loosens a gate). s636 passed with this change on GitHub (run 44879707201).
