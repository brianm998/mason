# Installing MASON

Three routes, one result: the `mason`, `masond`, `step-host` and `mason-mcp` binaries, and then
`mason init` in your project. None of them runs as root. The install script edits your shell profile only if you say yes (below).

| Route | Command | State |
| --- | --- | --- |
| Homebrew | `brew install mason` | A formula template is in `packaging/homebrew/mason.rb`; no tap or release exists yet |
| Install script | `curl -fsSL <release host>/install.sh \| bash` | `scripts/install.sh`, tested against a local fake release; no real release exists yet |
| Cargo | `cargo install mason-pipeline` | The crate is `publish = false` until Stage 31, so this does not work yet |

Until a release exists, build from source: `cargo build --release --workspace --bins` and put
`target/release` on your PATH.

## The install script

```text
bash install.sh [--version X.Y.Z] [--prefix DIR] [--base-url URL] [--profile|--no-profile]
```

It downloads `mason-<version>-<target>.tar.gz` and `SHA256SUMS` from the release folder, checks
the archive's SHA-256, and only then copies the binaries into `<prefix>/bin` (default
`~/.local/bin`). A mismatch, or a release with no checksum file, installs nothing.

**The `mason` shell function.** Several `mason` binaries can exist on one machine, and the one on
PATH is often not the one the daemon runs. When stdin is a terminal the script asks whether to add
a `mason()` function to your login shell's profile (zsh: `~/.zprofile`; bash: `~/.bash_profile` if
it exists, else `~/.profile`; anything else: `~/.profile`; the question names the file). The
function runs `<data dir>/runtimes/<contents of runtimes/current>/mason`, which self-update moves,
and falls back to `~/.config/mason/bin/mason` when `runtimes/current` is missing. The data dir is
`~/Library/Application Support/mason` on macOS and the XDG data dir on Linux. It is appended
between `# >>> mason >>>` and `# <<< mason <<<`; a block already there is replaced in place and
nothing outside the markers is touched. `--profile` says yes without asking, `--no-profile` says
no, and a non-interactive run (or `--yes` alone) edits nothing.

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

## Claude credentials (`claude.env`)

Agents use the login of the `claude` CLI, unless `~/.config/mason/claude.env` (beside the other
config files) exists. Put the token from `claude setup-token` in it and `chmod 600` it:

```sh
export CLAUDE_CODE_OAUTH_TOKEN=...
```

Only `ANTHROPIC_*` and `CLAUDE_*` lines are read, and nothing is expanded. The file is read each
time a session starts or resumes, so a new token needs no restart. When it sets a token or an API
key, any inherited from the daemon's environment is dropped; a project's `[agent_env]` still wins.

## Reaching the daemon away from home (Tailscale)

A phone paired on the home Wi-Fi finds the daemon as `<host>.local` (mDNS), which works only on that network. To check from elsewhere, put the Mac and the phone on a private overlay network such as Tailscale. Nothing is forwarded from your router and the daemon is never on the internet.

1. Install Tailscale on the Mac and on the phone and sign both in to the same tailnet.
2. In `daemon.toml`, turn on the LAN listener and name the Mac's tailnet name (or its `100.x.y.z` address):

   ```toml
   [lan]
   enabled = true
   hosts = ["mac.your-tailnet.ts.net", "100.101.102.103"]
   # bind = ["0.0.0.0"]   # the default; list addresses to listen on only those
   ```

3. Restart `masond`, run `mason lan status` to see the hosts and listening addresses, and pair the phone again with `mason pair`. The pairing URL now carries `host=<host>.local` and one `alt=` for each extra host, and the phone tries the local name first, then the others.

To listen on the Tailscale interface and loopback only, set `bind = ["127.0.0.1", "100.101.102.103"]`. The listener serves paired devices only, whichever address it is on.

## Where Add Project looks for repositories

The monitors' Add Project offers the git repositories one level under each root in `daemon.toml`. The default is `~/git`; `~` is your home directory. A root that cannot be read is reported next to the list and does not stop the others.

```toml
[onboarding]
roots = ["~/git", "~/work"]
```

Restart `masond` to apply a change.
