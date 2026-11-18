<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0592: smoke-list entries name cargo targets; modules of `it` use `--test it`

Smoke-list entries must name real cargo test targets. A module of the daemon's `it` executable is reached through `--test it`, which runs the whole executable, because the parser takes no name filter. `scripts/exit/s599.sh` enforces that every named target exists.
