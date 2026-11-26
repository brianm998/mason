# 1099: GitHub Actions are pinned by commit SHA

**Cause.** Actions were named by tag or branch. `release.yml` ran `softprops/action-gh-release@v2` with `contents: write`, and two workflows ran `dtolnay/rust-toolchain@master`: whatever those refs pointed at next would run with the repository's token.

**Decision.** Every third-party `uses:` names a full commit SHA with the release in a comment. `scripts/check-action-pins.sh` enforces it in CI's lint job; local `./` actions are exempt. Dependabot proposes new pins weekly as pull requests the owner reviews.

**Rejected.**
- Pinning only third-party publishers and leaving `actions/*` on tags: those tags move too, and one rule is simpler to check.
- Replacing `dtolnay/rust-toolchain` with `rustup` commands: changes behaviour; the pin is enough.
- Adding the check to MASON's own `lint` suite (`.mason/mason.toml`): not asked for.

**Trust files.** `.github/workflows/**`, `scripts/check-*.sh` and `scripts/exit/**` are protected; this change needs owner approval before landing.
