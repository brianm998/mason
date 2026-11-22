# 0795: the batch build floor runs on this Mac by default

Date: 2026-11-21. Decision: the owner approved this in chat (#816).

On 2026-11-21 between 13:33 and 13:54 three single-task batches (#535, #539, #540) failed after
waiting 1200 s for a GitHub runner, while this Mac was 3-10% busy. The floor is a short build in a
warm target directory, like the lint tier (#811), so it now runs here.

- `build_suites` (the floor and the base re-check) runs a suite's `build` in the batch worktree,
  under the `compile` slot it already took, with no placement (like lint).
- `floor_where = "local" | "github"` in `.mason/landing.toml` (top level, default local). With
  `github`, #815's placement (`place_of`) decides, as before.
- A suite whose `runs_on` names a runner that is not macOS stays where #815 puts it. This host is
  macOS, so `cargo-workspace` (no `runs_on`) and swift-clients (`macos-latest`) build here; a
  `ubuntu-*` or `windows-*` suite does not.
- The batch note says where and how long: `cargo-workspace: the batch builds (on this Mac, 1m05s)`;
  a red says `does not build (on this Mac, 20s)`.
- The batch test tiers are untouched.

Touches `crates/daemon/src/landing.rs` and `crates/landing/src/config.rs` (protected: verdict
logic). The commands and verdict are unchanged; only where the build runs.
