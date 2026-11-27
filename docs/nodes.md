<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# Running MASON work on other Macs (nodes)

A `masond` can **serve as a node**: it takes compile and test runs from another `masond` (the
**coordinator**), runs them on its own CPUs, and sends back what a local run would have produced.
This is how a second Mac (the Mac Studio) takes load off the iMac Pro. The design is in
`docs/multi-machine-nodes-plan.md`; this page is how to set it up and run it.

**What exists today:** the node listener, pairing and discovery; runs of suites, `run_check` and a
landing batch's tests on a node; draining, cancelling and giving up on a node; the monitor's switch
and Nodes section.

## How it fits together

- The **coordinator** is the `masond` that has your projects (the iMac). It finds nodes with
  mDNS, pairs with each one once, and asks each every few seconds: "can you take work?"
- A **node** is a `masond` with `[node] serve = true` (the Studio). It keeps **no tokens and no
  project configuration**. It remembers one thing: the fingerprint of each coordinator it
  trusts, written by pairing. Everything a job needs comes from the coordinator with the job.
- Every request is **mutual TLS**: each Mac's own certificate is its identity, and each side pins
  the other's SHA-256 fingerprint. There is nothing to type, copy or rotate after pairing.
- The Studio is your work machine, so it has an **on/off switch** ("accept work from other
  Macs"). It is **off** on a fresh install. Turning it off stops new jobs at once; jobs already
  running finish (`on_switch_off = "drain"`, the default).
- A node also reports why it is not taking work: too hot, the CPU busy with your own work, no
  free slot, low disk. It never queues a job: if it cannot start one at once, it says so and the
  coordinator tries another place (GitHub, or here).
- **What a job's checkout holds.** The snapshot's files, and `origin/<base>` (for example
  `origin/develop`) at the coordinator's base tip, so `git merge-base HEAD origin/develop` and
  `git diff origin/develop...` work as they do on the coordinator. There is no `origin` remote
  to fetch from and no other branch. The slots of a project share one mirror, so the ref is
  shared: a job running alone gets exactly its own base tip, and while jobs of the project
  overlap the ref only moves forward (a job may then see a newer base than its own). `mason`
  from the node's own runtime is first on `PATH` (so `mason definition check` works); it finds
  no daemon or configuration of the node, so commands that need `masond` report that it is not
  running. A coordinator or node from before this was added behaves as it did: no
  `origin/<base>`, and `mason` only if the node's own `PATH` has one.
- **Each project has one landing queue, on its coordinator.** A node never merges or pushes and
  holds no push credentials. Registering the same project on two coordinators is not supported.

## 1. Set up the Studio

Run a node as **its own macOS user**, so that a job (which is agent-written code being compiled
and tested) cannot read your files, Keychain or SSH keys. This is the only supported setup.

### 1.1 A standard user for the node

On the Studio, in an administrator's Terminal:

```bash
sudo sysadminctl -addUser masonnode -fullName "MASON node" -password "choose-a-password"
ls -ld /Users/masonnode
```

If `/Users/masonnode` does not exist yet, make it with `sudo createhomedir -c -u masonnode`.
The user does not need to log in, ever. To keep it off the login window:

```bash
sudo dscl . -create /Users/masonnode IsHidden 1
```

Do **not** make it an administrator.

### 1.2 Toolchains for that user

Everything a job compiles with comes from the node user's own home. Install what your projects
need, as that user:

```bash
sudo -u masonnode -H bash -lc 'curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain none'
sudo -u masonnode -H bash -lc 'rustup toolchain install 1.96.0'
sudo -u masonnode -H bash -lc 'rustup default 1.96.0'
```

Install the Rust version each project pins in its `rust-toolchain.toml` (MASON's own is 1.96.0).
The default matters: outside a project (which is where `mason node doctor` runs `cargo`), rustup
has no version to choose without it.
For Swift and iOS projects, Xcode is installed once for the whole Mac (from the App Store or as a
versioned `/Applications/Xcode_16.4.app`); select it for the node user in
`/Users/masonnode/.zprofile` with `export DEVELOPER_DIR=/Applications/Xcode_16.4.app/Contents/Developer`.
A user who has never used Xcode needs its license accepted and its first-launch components
installed, once, as an administrator (nobody can answer these later):

```bash
sudo xcodebuild -license accept
sudo xcodebuild -runFirstLaunch
```

Java, if a project needs it: install a JDK for the Mac; `/usr/libexec/java_home -V` lists it.
The coordinator only sends a job to a node whose toolchains cover what the job asks for.

### 1.3 Install the signed runtime and "MASON Daemon.app"

**The node never builds or signs MASON itself.** The signing key is in your login keychain on the
iMac, where `masonnode` cannot reach it, and an ad-hoc signature changes with every build, which
makes macOS forget every privacy permission you give the node, with nobody there to give it again.
So the iMac builds the runtime for the Studio's architecture, signs it with the `sign_identity` of
`update.toml`, and you carry it over once:

```bash
# On the iMac (a cross build: it needs `rustup target add aarch64-apple-darwin`, which it does):
M="$HOME/Library/Application Support/mason"
mason run --needs compile -- "$M/runtimes/$(cat "$M/runtimes/current")/mason" node export-runtime --arch arm64 --out ~/node-export
```

`mason run` starts a program file, and `mason` in your shell may be a shell function
(`scripts/install.sh` installs one), so the command after `--` is the runtime's own binary.

That writes `~/node-export/runtimes/<sha>/` (the signed binaries),
`~/node-export/MASON Daemon.app` (the app that macOS can give permissions to) and
`~/node-export/mason` (the node user's `mason` command). Copy the folder to the Studio, then, as an
administrator there:

```bash
sudo cp -R "node-export/MASON Daemon.app" /Applications/
sudo -u masonnode -H mkdir -p "/Users/masonnode/Library/Application Support/mason/runtimes"
sudo cp -R node-export/runtimes/* "/Users/masonnode/Library/Application Support/mason/runtimes/"
sudo chown -R masonnode "/Users/masonnode/Library/Application Support/mason"
# The runtime to start (the folder's name):
echo -n <sha> | sudo -u masonnode tee "/Users/masonnode/Library/Application Support/mason/runtimes/current"
# The node user's `mason` command, which the steps below use:
sudo -u masonnode -H mkdir -p /Users/masonnode/.local/bin
sudo install -o masonnode -m 755 node-export/mason /Users/masonnode/.local/bin/mason
```

`/Users/masonnode/.local/bin/mason` is a small script that sets the node's `PATH` and runs the
current runtime's `mason`, so it follows every update:

```sh
#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Installed as ~/.local/bin/mason on a MASON node (docs/nodes.md).
export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin"
runtimes="$HOME/Library/Application Support/mason/runtimes"
exec "$runtimes/$(cat "$runtimes/current")/mason" "$@"
```

From then on **updates are automatic**: `mason update` (and the monitor's "Update masond") brings the
iMac and every paired node to the same MASON version, and you do nothing by hand on the Studio.

What happens, in this order:

1. The iMac builds its own new runtime. Beside it, under their own compile leases and the same
   build-jobs cap, it cross-builds the same commit once for each node architecture it knows (the
   Studio's `arm64`), signed with `sign_identity`.
2. The iMac's build passes its self-test. Before it takes over, it sends each reachable node its
   runtime (`PUT /node/runtime`). The Studio checks the package against the team it pinned when you
   paired (`codesign --verify --strict`, and the team id of the signature), installs it, and takes over
   at once. Running jobs are not waited for: their commands run under step hosts, which the new
   daemon finds again as after any restart (only a job still being checked out is waited for, a few
   seconds, and the node takes no new job meanwhile). A package that is ad hoc,
   unsigned, from another team or broken is refused and nothing of it is installed.
3. The iMac takes over. Both Macs now run the commit, give or take the Studio's running jobs.

This works across a protocol bump: a node that speaks an older node protocol is listed as
`incompatible` and is sent the runtime all the same, which is how it stops being incompatible.

**What you see.** `mason update status` lists a line for each node, and so does `mason node list` (in
brackets) and the monitor's Nodes section:

| Shown | Meaning |
|---|---|
| `building for arm64` | the cross-build is running |
| `sending the new runtime` / `sent; installing` | on its way / the node has it |
| `installed; taking over now, 2 running jobs carry on` | installed; it takes over at once and its running jobs keep going under their step hosts |
| `up to date` | it runs the iMac's commit |
| `failed (reason)` | the send failed; an alert says so too |
| `will update when it returns` | it was off or silent; the first poll that finds it sends the runtime |

**If a node misses one.** A failed send is tried again by the poll every ten minutes. To send now:
`mason node update studio` (or `mason node update --all`; the monitor's Nodes row has the same
button). It builds if it has to, sends, and `mason node list` shows how it goes. Nothing here can
stop the iMac's own update: a node that cannot be reached is left for the next poll.

This needs `sign_identity` in the iMac's `update.toml`, and the iMac itself running a signed `masond`;
with neither the Studio is updated by hand, as above. `[nodes] push_runtime = false` turns the sending
off. The `release` channel has no source to cross-build from, so it does not update nodes yet.

**Both Macs must run the same MASON version** to work together: a node on another protocol takes no
work until it has been updated.

### 1.4 `daemon.toml`

As the node user, create `/Users/masonnode/.config/mason/daemon.toml`:

```toml
[node]
serve = true
# port = 7425            # the default; mDNS advertises whichever it is
# bind = ["0.0.0.0"]     # see "Which network" below
```

That is the whole configuration of a node. Its tunables (how many jobs, how much CPU of your
own work it tolerates, how much disk to keep free, the lease) are set **on the coordinator**
and sent to the node (see section 3). A node that has heard nothing uses safe defaults
(2 jobs, other work at most 50 % of the CPU, 50 GB free).

A node advertises the Bonjour service `_mason._tcp` as `MASON node on <host> (<8 characters of its
fingerprint>)`, and the LAN listener as `MASON on <host> (...)`. On macOS it registers through the
system's own responder, so it never claims the Mac's `.local` name. `MASON_NO_MDNS=1` in a
daemon's environment switches its advertising and browsing off, and every command a node runs has
it set.

**Keep the data directory's path short.** Each job's step host listens on a unix socket at
`<data-dir>/node/hosts/<16 characters>/sock`, and macOS limits the length of such a path (MASON
refuses one over 100 characters). So the data directory may be at most 67 characters; the one used
on this page, `/Users/masonnode/Library/Application Support/mason`, is 51. `mason node doctor`
checks it. A node running an older MASON put the job's whole id in the path, and could run no job
under that directory: update it, or until then give it a short one (`/Users/masonnode/m`).

**Keep the node's data on the Studio's internal disk** (the default: everything is under
`/Users/masonnode`: mirrors, slots, `~/.cache/mason-target`, cargo). Then no volume permission is
needed, there are no external-disk stalls, and it is the fastest. Do not put slots or target
directories on a network share: a share you mount in your own session is yours and vanishes when
you log out, and cargo's file locking is slow and unreliable over SMB. If you really need another
volume, see "Privacy grants".

### 1.5 Start it at boot, as that user

A node must run without anyone logged in, so it is a **LaunchDaemon** with `UserName`, not a
LaunchAgent. The program is **inside MASON Daemon.app**, so that the permissions you give the app
(next section) cover it. Save this as `/Library/LaunchDaemons/com.mason.node.plist` (owned by
`root:wheel`, mode 644):

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.mason.node</string>
  <key>UserName</key>
  <string>masonnode</string>
  <key>ProgramArguments</key>
  <array>
    <string>/Applications/MASON Daemon.app/Contents/MacOS/masond</string>
    <string>--managed</string>
    <string>--data-dir</string>
    <string>/Users/masonnode/Library/Application Support/mason</string>
    <string>--config-dir</string>
    <string>/Users/masonnode/.config/mason</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>HOME</key>
    <string>/Users/masonnode</string>
    <key>PATH</key>
    <string>/Users/masonnode/.cargo/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
  <key>StandardOutPath</key>
  <string>/Users/masonnode/Library/Application Support/mason/masond.out.log</string>
  <key>StandardErrorPath</key>
  <string>/Users/masonnode/Library/Application Support/mason/masond.err.log</string>
</dict>
</plist>
```

```bash
sudo chown root:wheel /Library/LaunchDaemons/com.mason.node.plist
sudo chmod 644 /Library/LaunchDaemons/com.mason.node.plist
sudo launchctl bootstrap system /Library/LaunchDaemons/com.mason.node.plist
```

Because it is a daemon there is no GUI session for that user: nothing a job does can appear on
your screen, and jobs never run UI tests or wake the display.

### 1.6 Privacy grants, given once, in your own session

A node has nobody to answer a macOS privacy prompt: the request is denied, and the process gets
"operation not permitted" or, at worst, hangs. So the node never *needs* a prompt: you grant
everything ahead of time, to **MASON Daemon.app**, whose signature does not change across updates.
In System Settings on the Studio, as an administrator:

- **Privacy & Security → Developer Tools:** add MASON Daemon.app. It skips the malware scan that
  macOS runs on every launch of freshly built code (a pipeline launches a great deal of it).
  If this grant turns out to belong to one user and not to the whole Mac, log in as `masonnode`
  **once** (fast user switching), add it there, and log out.
- **Privacy & Security → Full Disk Access:** add MASON Daemon.app **only if** the node must use a
  removable or network volume. It is stored for the whole Mac and covers `masond` and everything it
  starts, whichever user runs it. (Check that it really covers a LaunchDaemon running as
  `masonnode`: `mason node doctor` says so, below.)
- **Nothing else.** No Accessibility, Screen Recording, Input Monitoring, Automation or camera: a
  node runs no UI tests, so it never needs them.
- **Local Network** (macOS 15 and later): launchd *daemons* are not subject to it, so the Studio
  needs nothing. The **iMac** does: see below.

**An external disk.** macOS mounts an external disk only after somebody logs in, unless you say
otherwise; and `masonnode` must be able to write to it (give it "Ignore ownership on this volume",
or a directory it owns):

```bash
sudo defaults write /Library/Preferences/SystemConfiguration/autodiskmount AutomountDisksWithoutUserLogin -bool true
```

**A managed Mac.** If the Studio is managed by an employer (MDM), a profile may block or pre-set
these permissions. Check System Settings → Privacy & Security → Profiles first.

**On the iMac (the coordinator): Local Network.** The first time MASON connects to a node, macOS
asks "Allow MASON to find devices on local networks?". Allow it: the answer is kept across updates
because MASON is signed by your team and runs from one path (see "The coordinator: Local Network"
below). If it was ever answered "Don't Allow" (a scratch test daemon
raised the same question on 2026-11-22 for the same app), turn it back on in System Settings →
Privacy & Security → Local Network. `mason node list` says "local network access denied by macOS"
for a node it cannot reach while that is off. ("No route to host" is also what a node that is off
the network gives, so check the node is up first.)

### 1.7 `mason node doctor`

Run it on the Studio, as `masonnode`, until it is clean:

```bash
sudo -u masonnode -H /Users/masonnode/.local/bin/mason node doctor
```

It checks, and the node repeats the checks when it starts and every ten minutes, so a grant that
is lost shows up before a job meets it:

- **signature:** masond is signed by a team (not ad hoc, not unsigned);
- **paths:** it can create, write, read and delete a file in every directory it uses (data,
  mirrors, slots, the build cache, cargo), within ten seconds each, so a stalled disk or a denial
  is reported and never hangs it; a denied volume says "give MASON Daemon.app Full Disk Access";
- **volumes:** a volume it uses is mounted;
- **socket path:** the sockets of its jobs' step hosts fit under the data directory;
- **listener and mDNS:** the node listener is bound, and mDNS is advertising (a node that is not
  advertised can still be added by address);
- **toolchains:** `cargo`, `xcode-select` and `java` run as that user without a prompt, found on
  the `PATH` a job gets (`~/.cargo/bin` first, whatever the daemon's own `PATH` is) and run from the
  user's home. A `cargo` with no default toolchain says to run `rustup default <version>`.
- **xctest:** `com.apple.testmanagerd.control` can be looked up from the node's launchd domain.
  testmanagerd is a per-login-session agent (`LimitLoadToSessionType` is `LoginWindow` and `Aqua`),
  so a LaunchDaemon with nobody logged in as the node user has no such service, and
  `xcodebuild test` ends in "Connection init failed at lookup with error 3" a couple of seconds
  after the build. This does not stop the node (builds work); the node reports an empty `xctest`
  toolchain, and the coordinator sends no `xcodebuild test` run to it. Hosted macOS app tests also
  need a window server connection, so the only fix is a GUI session for the node user; with
  FileVault on, that means someone logging in at the console after each boot.

A failure that stops the node doing a job makes it `not accepting`, with the reason as `mason node
list` shows it on the iMac. A new tool that does not run as `masonnode` is almost always a missing
license or first-launch step (section 1.2).

### 1.8 The firewall

If the macOS application firewall is on, let the node daemon take incoming connections
(otherwise the Mac shows a dialog the first time, which nobody answers):

```bash
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add "/Applications/MASON Daemon.app/Contents/MacOS/masond"
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp "/Applications/MASON Daemon.app/Contents/MacOS/masond"
```

**Which network.** The node listens on every interface by default (`bind = ["0.0.0.0"]`). If
the Studio is also on a VPN or a guest network, name only the LAN address:
`bind = ["192.168.1.40"]`. The listener serves `/node/*` only, and only to certificates a
pairing has trusted; anyone else gets a refusal before a request is read.

## The coordinator: Local Network, and running `masond` from the internal disk

macOS (15 and later) enforces Local Network permission by the executable's Mach-O UUID. It keeps
one rule per executable path, made when it shows the prompt, and it has to be able to read that
path to find the UUID. For a bare `masond` in `runtimes/<sha>/` on an external disk it never
could: on 2026-11-25 the iMac's `masond` got "no route to host" (errno 65) for every connection to
the LAN, while ping and Terminal reached the node and System Settings showed every `masond` entry
allowed.

**The log line to look for:**

```bash
log show --last 3h --predicate 'process == "nehelper"' | grep "0 UUID(s) for com.brianm998.mason.masond"
```

(Nearby: `Local network allowed by preference for masond (...), but received prompt` and
`UUID cache miss ... Failed to find com.brianm998.mason.masond using neagent`.)

**What MASON does.** The coordinator execs `masond` from one signed bundle whose path never
changes, `~/Applications/MASON Coordinator.app` (bundle identifier `com.brianm998.mason.masond`,
the identifier `masond` is signed with; not `MASON.app`, which is the monitor). Every start and
every takeover puts that runtime's `masond`, `step-host`, `mason-mcp` and other binaries into
`Contents/MacOS`, signs the bundle with `sign_identity` and verifies it, then execs it. The bundle
it replaces is kept as `.MASON Coordinator.previous`. The runtimes stay in the data directory,
wherever that is. The launch agent does not change.

This happens when all of these hold: macOS, `sign_identity` is set in `update.toml`, the data
directory is the default `~/Library/Application Support/mason` (or a link to the same place), and
this daemon is not a node. `app_dir` in `update.toml` overrides the rule: an absolute (or `~/`)
directory on the internal disk to keep the bundle in (use it if the home directory itself is
external), or `"off"`. If the bundle cannot be made, `masond` logs a warning and runs from the
runtime directory as before. At start-up it also warns if its executable is on a volume that is
not internal.

**After the first takeover with it,** `ps -o command= -p <pid in daemon.json>` should show the
bundle's path. If macOS asks about the local network, allow it. If it does not ask and
connections still fail, check the log line above: the old rules for the same identifier may still
be what macOS finds. Run
`/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f ~/Applications/"MASON Coordinator.app"`
and restart `masond`. (Not yet confirmed on a real machine.)

**What MASON says.** When a connection to a node on the LAN fails with "no route to host", the node
answers ping and `masond` is on an external disk, `mason node list`, `mason node add` and the
monitor say: "macOS has a Local Network rule for masond but cannot apply it (the binary lives on an
external disk). Run MASON from the internal disk (see docs/nodes.md)". Otherwise they say "local
network access denied by macOS". mDNS send failures show only in the daemon's log
(`mdns_sd ... No route to host`).

**Stopgap: a loopback relay.** Local Network privacy does not cover loopback, and Terminal has the
permission. Run a small TCP relay in Terminal that listens on `127.0.0.1:17425` and forwards to the
node's port (7425), and pair through it; keep it running. The node's certificate is still pinned
end to end:

```bash
python3 - <<'PY'
import asyncio

NODE = ("10.10.10.5", 7425)  # the node

async def pipe(r, w):
    try:
        while data := await r.read(65536):
            w.write(data)
            await w.drain()
    finally:
        w.close()

async def serve(r, w):
    nr, nw = await asyncio.open_connection(*NODE)
    await asyncio.gather(pipe(r, nw), pipe(nr, w), return_exceptions=True)

async def main():
    async with await asyncio.start_server(serve, "127.0.0.1", 17425) as s:
        await s.serve_forever()

asyncio.run(main())
PY
```

```bash
mason node add --host 127.0.0.1:17425 --fp FINGERPRINT --code CODE --name studio
```

Remove the node and pair it by its own address once the coordinator can reach it.

## 2. Pair the iMac with the Studio

Pairing is done once. The code lasts **2 minutes** and works once; five wrong guesses lock it.

1. On the Studio, as the node user (the data directory is the node user's, so run the `mason`
   command as that user):

   ```bash
   sudo -u masonnode -H /Users/masonnode/.local/bin/mason node pair-code
   ```

   It prints the code, the node's fingerprint and its port.

2. On the iMac:

   ```bash
   mason node add studio --code 12345678
   ```

   `studio` is the name mDNS shows; `mason node list` lists what it can see. If mDNS cannot see the
   Studio (a VPN, Wi-Fi client isolation), give its address and the fingerprint the Studio printed:

   ```bash
   mason node add --host 192.168.1.40:7425 --fp <fingerprint> --code 12345678
   ```

   You can also list such a node in `daemon.toml` as `[nodes] static = ["192.168.1.40:7425"]` so
   `mason node list` shows it before it is paired.

   The iMac checks that the certificate the Studio shows is the one in its mDNS record (or the
   fingerprint you typed) **before** it sends the code. Pairing also tells the Studio the team the
   iMac's MASON is signed by; that team is the only one whose runtime the Studio will install.

3. Check:

   ```bash
   mason node list
   ```

   shows the Studio as `not accepting (the owner turned remote work off)`: the switch is off.

`mason hosts`, on the coordinator, shows this Mac, each node and GitHub with what runs on each
now (a node reads as accepting, full, off or absent), and `mason task show` says where a task is
running.

## 3. Turning it on and off

On the Studio, the switch is one command, run as the node user:

```bash
sudo -u masonnode -H /Users/masonnode/.local/bin/mason node accept on
sudo -u masonnode -H /Users/masonnode/.local/bin/mason node accept off
```

`accept off` stops new jobs at once. The same switch is in the monitor (next section).
Jobs already running finish by default. `mason node stop-jobs` cancels them immediately, for the
moment you need the machine back; the coordinator then runs those jobs somewhere else.

### The monitor

- On a Mac that serves as a node, Home and Settings show **Accept work from other Macs**: one click
  turns the switch on or off, and the line under it says what the node is doing ("Off. 2 jobs still
  running here will finish.", "On: taking work from other Macs.", or why it is held: "too hot: die
  96 °C").
- On a coordinator, Home shows a **Nodes** section: a "Send runs to nodes" switch (the same as
  `mason node use`), how many paired nodes take work, and a row per node with its state and its
  CPU, die temperature and jobs. The old GitHub section is now **Remote runs**, and a run on a node
  is a row in it ("on node studio").
- **Reaching the node's daemon from your own login.** The node daemon belongs to `masonnode`, so
  the monitor you run as yourself cannot read its `daemon.json`. Give the node daemon a fixed port in
  its `daemon.toml` (`[daemon]` `port = 7433`) and start the monitor on the Studio with
  `--port 7433`. That port is on loopback only. Loopback is trusted by design, so any process on the
  Studio can reach it, including a job: a job can turn the switch off or on for the machine it runs on,
  and nothing else of yours is reachable through it. The node's listener for coordinators is a
  separate port that only paired coordinators can use.

On the iMac, `mason node use on` lets runs go to nodes (`[nodes] enabled = true`, written to
`daemon.toml` and read live) and `mason node use off` stops it. Nodes are tried before GitHub
(`prefer = "nodes"`).

The coordinator's settings for a node are in the iMac's `daemon.toml`, read live, and sent to the
node with each status poll:

```toml
[nodes]
enabled = true
# prefer = "nodes"        # or "github"
# static = ["studio.local:7425"]
# projects = ["mason"]    # only these projects may use nodes; default: all
# status_every_s = 5
# lost_after_polls = 6

[nodes.default]           # every node
max_jobs = 4
max_other_cpu_pct = 50    # takes no job while your own work uses more of its CPU than this
min_disk_free_gb = 50
on_switch_off = "drain"   # or "cancel"
lease_s = 300

[nodes.studio]            # one node, by the name in `mason node list`
max_jobs = 6
```

The node keeps what it was sent in memory only. When several coordinators are paired, the
strictest value of each setting applies.

## 4. Taking a coordinator away

On the Studio: `mason node coordinators remove <name>` (as the node user). On the iMac:
`mason node remove studio`. If the iMac's certificate is ever replaced (a new data directory),
pair again.

## Projects without GitHub Actions

A project needs no `[remote_runs]`, token or repository on GitHub to use nodes. Give it a placement
mode of its own:

```bash
mason node use on                              # once: runs may go to nodes
mason remote mode remote --project P           # or the monitor's Settings, "Where runs go"
mason remote status                            # P   remote (nodes only: no GitHub setup)
```

This writes to `~/.config/mason/projects/P.toml` (and re-reads it; edit it by hand and run `mason
project reload --project P` for the same effect):

```toml
[placement]
mode = "remote"      # or "favor-remote", "favor-local", "local"
```

| mode | a node has room | no node has room |
| --- | --- | --- |
| `remote` | the node | waits for one; never runs on the coordinator |
| `favor-remote` | the node | runs on the coordinator, under its usual limits |
| `favor-local` | the coordinator, a node while it is overloaded | the coordinator |
| `local` | the coordinator | the coordinator |

- A project with no mode always runs on the coordinator. Clearing the mode returns it to that.
- A suite with `remote = false`, one whose environment leases a device or simulator, and one that
  uses `exclusive` always run on the coordinator.
- A node takes a suite only if it has its toolchain: Xcode for a suite named `swift*`, Java for
  `compose*`, otherwise Rust; and the Xcode version the command names, if it names one.
- A `remote` project waits while nodes are off or no node has the toolchain. The daemon log says
  "waiting for a node".
- Baseline runs, and landing's lint tier and build floor unless the definition sends them away,
  still run on the coordinator.
- A project with `[remote_runs]` keeps using `[remote_runs] mode`, which wins over `[placement]`.

## A node's git state

A node's mirrors (`<data>/node/mirrors/<coordinator>/<project>.git`) have no remote: they only
get commits as bundles from the coordinator, so a `git fetch` or `git pull` there does nothing.
MASON keeps them itself. Each job sends the project's base, and the node sets `origin/<base>`
before the command runs. A finished run says which (`ran on node studio (arm64), 3 min, base: set
develop@0123456789`).

When the node cannot set it, or the coordinator cannot resolve it, the run is a node failure,
never a red for the task: `origin/develop not set on node studio (base: failed: ...)`. The run
goes to the next place; for a project that only runs on nodes it is retried later.

On the coordinator:

```
mason node show studio                        # each mirror, its path and its origin/<name> commits
mason node sync studio --project mason        # send the base again and re-set origin/<base>
mason node sync studio --project mason --base release/1.0
```

`sync` sends a bundle of the base's whole history, as the coordinator's clone has it (it fetches
nothing), and prints where `origin/<base>` is in the node's mirror. If git refuses (a `.lock` file
left in the mirror, a full disk), it prints git's own words. While a job of the project runs on the
node the ref only moves forward. Both commands need a node on this runtime (`mason node update`).

## When something goes wrong

- **You turn the switch off.** No new job starts. Jobs already running finish and the iMac collects
  them as usual (`on_switch_off = "drain"`). If you need the machine back at once: `mason node
  stop-jobs` (or set `on_switch_off = "cancel"` for that node on the iMac, which does it whenever
  the switch goes off). Where the job goes next depends on the project's mode, and no task is charged:
  with `remote` and `fallback` not `local`, a suite (a task's test step or a landing batch's) never
  starts on the iMac. It goes to GitHub if that can take it, else to a node with room, else waits
  with backoff for a node (a skip or a daemon stop ends the wait), and with no place that can ever
  take it, or a node without the base, it is reported as an outage, never a red. With
  `fallback = "local"` or a `favor-*` mode it runs on the iMac under the usual slots after GitHub
  and other nodes were tried.
- **The Studio sleeps or leaves the network.** After `lost_after_polls` polls in a row with no
  answer the iMac gives up on the job and places it again by the same rule as above. The Studio, when it is back, kills
  the orphan once its lease (`lease_s`) has run out with nobody asking after it.
- **The node is too hot or busy with your own work.** It takes no new job; a job already running
  continues, since there is no pre-emption.
- **The iMac restarts mid-run.** It finds the record of the run it sent and asks the Studio for that
  job; nothing is sent again.

## Troubleshooting

- **`not accepting (masond is signed ad hoc ...)`.** The Studio is running a binary nobody with the
  team's key signed (a development build, or one built on the Studio). Install the runtime the iMac
  exports (section 1.3). For a development node only, `MASON_NODE_ALLOW_UNSIGNED=1` in the daemon's
  environment skips this check; its privacy grants will not survive an update.
- **`macOS denied access to /Volumes/X: give MASON Daemon.app Full Disk Access`.** Add the app under
  Privacy & Security → Full Disk Access (section 1.6), or move the node's data to the internal disk.
- **`step host exited before it was up (...)`.** The text after it is what the host said; all of it
  is in `host.err` in the job's folder under `<data-dir>/node/hosts/`, where the file `name` says
  which job and phase the folder is for.
- **`socket path too long`.** The data directory's path is too long (section 1.4).
- **`/Volumes/X is not mounted`.** The disk mounts only after a login unless
  `AutomountDisksWithoutUserLogin` is set (section 1.6).
- **`a runtime from this coordinator cannot be checked`.** The iMac's `masond` is not signed by a
  team, so it cannot send a runtime; update the Studio by hand (section 1.3).
- **`local network access denied by macOS` on the iMac.** Turn Local Network on for MASON (section
  1.6). If it is already on, see "The coordinator: Local Network".
- **`macOS has a Local Network rule for masond but cannot apply it`.** See "The coordinator: Local
  Network".

- **`mason node add` cannot find the Studio.** mDNS is blocked on some networks (a VPN, Wi-Fi
  isolation, a guest network). Use `--host ADDRESS:7425 --fp FINGERPRINT`, and consider
  `[nodes] static`.
- **"that node's certificate is not the one paired".** The Studio's `masond` was reinstalled or
  its data directory was cleared, which made it a new certificate. Remove it
  (`mason node remove studio`) and pair again.
- **`not paired (403)`.** The Studio no longer trusts this iMac (its `coordinators.json` was
  cleared, or the coordinator was removed there). Pair again.
- **`incompatible`.** The two Macs run different MASON versions. `mason update` sends the node the
  runtime itself; if it is still `failed` or `will update when it returns`, `mason node update NAME`
  sends it now, and `mason update status` says why it did not go.
- **A new binary hangs at 0 % CPU on its first launch.** macOS checks every new binary
  (`syspolicyd`). It has been slow to do so on the iMac; give it a few minutes, and check with
  `/usr/bin/pgrep -x syspolicyd`.
- **Nothing is listening.** `mason node status` (as the node user) says whether the daemon is
  serving. Check `[node] serve = true` and that `masond` was restarted after the change, and the
  daemon's log: `/Users/masonnode/Library/Application Support/mason/masond.err.log`.
