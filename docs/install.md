# Installing MASON

Three routes, one result: the `mason`, `masond`, `step-host` and `mason-mcp` binaries, and then
`mason init` in your project. None of them runs as root or edits your shell profile.

| Route | Command | State |
| --- | --- | --- |
| Homebrew | `brew install mason` | A formula template is in `packaging/homebrew/mason.rb`; no tap or release exists yet |
| Install script | `curl -fsSL <release host>/install.sh \| bash` | `scripts/install.sh`, tested against a local fake release; no real release exists yet |
| Cargo | `cargo install mason-pipeline` | The crate is `publish = false` until Stage 31, so this does not work yet |

Until a release exists, build from source: `cargo build --release --workspace --bins` and put
`target/release` on your PATH.

## The install script

```text
bash install.sh [--version X.Y.Z] [--prefix DIR] [--base-url URL]
```

It downloads `mason-<version>-<target>.tar.gz` and `SHA256SUMS` from the release folder, checks
the archive's SHA-256, and only then copies the binaries into `<prefix>/bin` (default
`~/.local/bin`). A mismatch, or a release with no checksum file, installs nothing.

**What the checksum does and does not prove.** It shows the download matches the release's own
checksum file. It does not show the release is the owner's: that is the signature. The script
verifies `SHA256SUMS.minisig` only when `minisign` is installed and `MASON_MINISIGN_PUBKEY` is
set; otherwise it prints `signature NOT verified`. No release is signed yet (Stage 31), so
today it always prints that.

## A `mason` that is not MASON

Dart's `mason_cli` also installs a command called `mason`. After installing, the script asks the
first `mason` on your PATH what it is (it runs `--help` and reads the first line, rather than
guessing from the name) and, if it is not this program, says so with the ways out:

- put MASON's folder first on PATH: `export PATH="$HOME/.local/bin:$PATH"`
- or call it by full path, or an alias: `alias mason-pipeline=$HOME/.local/bin/mason`
- or remove the other one: `dart pub global deactivate mason_cli`

`mason doctor` makes the same check at any time.

## First run

```text
cd my-project
mason init            # the eight questions; or: mason init --workflow solo --yes
git diff main...mason/setup && git checkout main && git merge --ff-only mason/setup
masond &              # restart it if it was already running: it reads projects at start
mason init --prove-only
```

`mason init` writes `.mason/` and the "Filing work" notes on a new branch, `mason/setup`, made in a
temporary worktree: your working tree, your current branch and your remote are not touched. You
merge it when you have read it. See `mason --help` for every flag; each question has one.

`mason adopt` registers a project that already has a `.mason/` (it never writes `.mason/`).
`mason remove --yes` deletes the project file, `.mason/` and the notes, and leaves tracker tasks,
git branches and machine-wide settings alone.

The example workflows are in `templates/` (MIT licensed, so copying them creates no obligation).
