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
```

Install the Rust version each project pins in its `rust-toolchain.toml` (MASON's own is 1.96.0).
For Swift and iOS projects, Xcode is installed once for the whole Mac (from the App Store or as a
versioned `/Applications/Xcode_16.4.app`); select it for the node user in
`/Users/masonnode/.zprofile` with `export DEVELOPER_DIR=/Applications/Xcode_16.4.app/Contents/Developer`.
Java, if a project needs it: install a JDK for the Mac; `/usr/libexec/java_home -V` lists it.
The coordinator only sends a job to a node whose toolchains cover what the job asks for.

Start `masond` once by hand as that user and let macOS check the new binaries: the first launch
of any binary it has not seen can take a minute or more (see "Troubleshooting").

### 1.3 Install `masond` for that user

The simplest route is the install script, run as the node user (see `docs/install.md`):

```bash
sudo -u masonnode -H bash -c 'bash install.sh'
```

or copy the four binaries (`mason`, `masond`, `step-host`, `mason-mcp`) built from the same
MASON version as the coordinator's into `/Users/masonnode/.local/bin`. **Both Macs must run the
same MASON version**: a node that speaks another node protocol is listed as incompatible and
never used. Each Mac updates itself with `mason update`; there is no remote update of nodes.

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

### 1.5 Start it at boot, as that user

A node must run without anyone logged in, so it is a **LaunchDaemon** with `UserName`, not a
LaunchAgent. Save this as `/Library/LaunchDaemons/com.mason.node.plist` (owned by `root:wheel`,
mode 644):

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
    <string>/Users/masonnode/.local/bin/masond</string>
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

### 1.6 The firewall

If the macOS application firewall is on, let the node daemon take incoming connections
(otherwise the Mac shows a dialog the first time, which nobody answers):

```bash
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --add /Users/masonnode/.local/bin/masond
sudo /usr/libexec/ApplicationFirewall/socketfilterfw --unblockapp /Users/masonnode/.local/bin/masond
```

**Which network.** The node listens on every interface by default (`bind = ["0.0.0.0"]`). If
the Studio is also on a VPN or a guest network, name only the LAN address:
`bind = ["192.168.1.40"]`. The listener serves `/node/*` only, and only to certificates a
pairing has trusted; anyone else gets a refusal before a request is read.

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
   fingerprint you typed) **before** it sends the code.

3. Check:

   ```bash
   mason node list
   ```

   shows the Studio as `not accepting (the owner turned remote work off)`: the switch is off.

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

## When something goes wrong

- **You turn the switch off.** No new job starts. Jobs already running finish and the iMac collects
  them as usual (`on_switch_off = "drain"`). If you need the machine back at once: `mason node
  stop-jobs` (or set `on_switch_off = "cancel"` for that node on the iMac, which does it whenever
  the switch goes off). The iMac then runs those jobs somewhere else, and no task is charged.
- **The Studio sleeps or leaves the network.** After `lost_after_polls` polls in a row with no
  answer the iMac gives up on the job and runs it somewhere else. The Studio, when it is back, kills
  the orphan once its lease (`lease_s`) has run out with nobody asking after it.
- **The node is too hot or busy with your own work.** It takes no new job; a job already running
  continues, since there is no pre-emption.
- **The iMac restarts mid-run.** It finds the record of the run it sent and asks the Studio for that
  job; nothing is sent again.

## Troubleshooting

- **`mason node add` cannot find the Studio.** mDNS is blocked on some networks (a VPN, Wi-Fi
  isolation, a guest network). Use `--host ADDRESS:7425 --fp FINGERPRINT`, and consider
  `[nodes] static`.
- **"that node's certificate is not the one paired".** The Studio's `masond` was reinstalled or
  its data directory was cleared, which made it a new certificate. Remove it
  (`mason node remove studio`) and pair again.
- **`not paired (403)`.** The Studio no longer trusts this iMac (its `coordinators.json` was
  cleared, or the coordinator was removed there). Pair again.
- **`incompatible`.** The two Macs run different MASON versions: update the older one
  (`mason update`).
- **A new binary hangs at 0 % CPU on its first launch.** macOS checks every new binary
  (`syspolicyd`). It has been slow to do so on the iMac; give it a few minutes, and check with
  `/usr/bin/pgrep -x syspolicyd`.
- **Nothing is listening.** `mason node status` (as the node user) says whether the daemon is
  serving. Check `[node] serve = true` and that `masond` was restarted after the change, and the
  daemon's log: `/Users/masonnode/Library/Application Support/mason/masond.err.log`.
