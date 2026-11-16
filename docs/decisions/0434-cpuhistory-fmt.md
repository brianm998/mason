# 0434: cpuhistory test filter formatted by rustfmt

The three-line `.filter(|p| { ... })` in the cpuhistory test is joined onto one line, which is what rustfmt produces and fits in 100 columns. Only `cargo fmt --all` output is applied; no logic changes.
