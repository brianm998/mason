# Contributing to MASON

Contributions arrive as GitHub pull requests and are reviewed by the owner.

## Developer Certificate of Origin

Every commit must carry a sign-off certifying the
[DCO](https://developercertificate.org/):

```bash
git commit -s
```

This adds `Signed-off-by: Your Name <you@example.com>`. CI rejects commits
without it. There is no CLA.

## Checks that must pass

`cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace`, `cargo deny check`, `scripts/check-spdx.sh`,
`scripts/check-dco.sh`, and the exit scripts in `scripts/exit/`. The full local
suite must stay under 10 minutes.

## Source headers

Every source file starts with `SPDX-License-Identifier: GPL-3.0-or-later`.
Files under `templates/` use the permissive licence stated in
[docs/LICENSING.md](docs/LICENSING.md) instead.

## Rules of the codebase

See Part 1 of [docs/DESIGN.md](docs/DESIGN.md). In particular: the reducer is
pure, blocking work never runs on the async event loop, and tests never reach
real services.
