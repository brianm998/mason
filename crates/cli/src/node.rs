// SPDX-License-Identifier: GPL-3.0-or-later

//! `mason node ...`: the commands of running work on other Macs (`docs/nodes.md`). On the node:
//! `pair-code`, `accept on|off`, `status`, `jobs`, `stop-jobs`, `coordinators [remove NAME]`.
//! On the coordinator: `list`, `add`, `remove`, `use on|off`. Each is one call to the daemon of
//! the data directory in use, so on a node that runs as another macOS user, run them as that
//! user (`sudo -u masonnode mason node ...`, see docs/nodes.md).

use crate::client::Daemon;
use crate::{die, finish, or_die, Args};
use serde_json::{json, Value};

pub const COMMANDS: &str = "node pair-code | accept on|off | status | doctor | jobs | stop-jobs | coordinators [remove NAME]  (on a node)\n       node list | add [NAME | --host H:PORT --fp FINGERPRINT] [--code C] [--name N] | remove NAME | use on|off | update NAME|--all | export-runtime --arch ARCH --out DIR  (on the coordinator)";

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or("?")
}

fn on_off(a: &mut Args, what: &str) -> bool {
    match a.pos().as_deref() {
        Some("on") => true,
        Some("off") => false,
        _ => die(&format!("{what}: say `on` or `off`")),
    }
}

fn as_owner() {
    // Changing what a machine does is the owner's act: say who.
    if std::env::var("MASON_AS").is_err() {
        if let Ok(u) = std::env::var("USER") {
            std::env::set_var("MASON_AS", u);
        }
    }
}

fn pct(v: &Value) -> String {
    v.as_u64().map_or("?".into(), |p| format!("{p}%"))
}

pub fn run(a: &mut Args, d: &Daemon, json_out: bool) {
    let sub = a
        .pos()
        .unwrap_or_else(|| die(&format!("which node command?\n       {COMMANDS}")));
    match sub.as_str() {
        "pair-code" => {
            finish(a);
            let r = or_die(d.call("POST", "/node/pair-code", Some(&json!({}))));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "Pairing code: {}   (valid {} s, used once)",
                s(&r["code"]),
                r["expires_in_s"]
            );
            println!("Fingerprint:  {}", s(&r["fingerprint"]));
            println!("Port:         {}", r["port"]);
            println!(
                "\nOn the coordinator:  mason node add {} --code {}\n  or, if mDNS cannot see this Mac:\n  mason node add --host <address>:{} --fp {} --code {}",
                s(&r["host"]).trim_end_matches(".local"),
                s(&r["code"]),
                r["port"],
                s(&r["fingerprint"]),
                s(&r["code"]),
            );
        }
        "accept" => {
            let on = on_off(a, "mason node accept");
            finish(a);
            as_owner();
            let r = or_die(d.call("POST", "/node/accept", Some(&json!({"on": on}))));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "This Mac {} work from other Macs. Running jobs are not stopped (`mason node stop-jobs` does that).",
                if on { "now accepts" } else { "no longer accepts new" }
            );
        }
        "status" => {
            finish(a);
            let r = or_die(d.call("GET", "/node/status", None));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "{} ({} {}, {} cores): {}",
                s(&r["name"]),
                s(&r["arch"]),
                s(&r["os"]),
                r["cores"],
                if r["accepting"] == true {
                    "accepting work".to_string()
                } else {
                    format!("not accepting: {}", s(&r["why_not"]))
                }
            );
            println!(
                "switch {}   CPU {} (other work {})   die {}   disk {} GB free   jobs {} of {}",
                if r["switch"]["on"] == true {
                    "on"
                } else {
                    "off"
                },
                pct(&r["cpu_pct"]),
                pct(&r["other_cpu_pct"]),
                r["thermal"]["die_c"]
                    .as_f64()
                    .map_or("?".into(), |c| format!("{c:.0} °C")),
                r["disk_free_gb"],
                r["jobs"]["running"],
                r["jobs"]["max"],
            );
            println!("serving as a node: {}", r["serving"]);
            println!("settings in force: {}", r["settings"]);
            let cs = r["coordinators"].as_array().cloned().unwrap_or_default();
            if cs.is_empty() {
                println!("no coordinator is paired (`mason node pair-code`)");
            }
            for c in cs {
                println!("coordinator {}", s(&c["name"]));
            }
        }
        "doctor" => {
            finish(a);
            let r = or_die(d.call("POST", "/node/doctor", Some(&json!({}))));
            if json_out {
                println!("{r:#}");
            } else {
                for c in r["checks"].as_array().cloned().unwrap_or_default() {
                    let mark = match (c["ok"] == true, c["blocks"] == true) {
                        (true, _) => "ok  ",
                        (false, true) => "FAIL",
                        (false, false) => "note",
                    };
                    println!("{mark} {:<13} {}", s(&c["name"]), s(&c["detail"]));
                }
                println!(
                    "{}",
                    if r["ok"] == true {
                        "This node can take work."
                    } else {
                        "This node will not take work until the FAIL lines are fixed (docs/nodes.md)."
                    }
                );
            }
            if r["ok"] != true {
                std::process::exit(1);
            }
        }
        "jobs" => {
            finish(a);
            let r = or_die(d.call("GET", "/node/jobs", None));
            if json_out {
                return println!("{r:#}");
            }
            let jobs = r["jobs"].as_array().cloned().unwrap_or_default();
            if jobs.is_empty() {
                println!("no jobs are running here");
            }
            for j in jobs {
                println!("{j}");
            }
        }
        "stop-jobs" => {
            finish(a);
            as_owner();
            let r = or_die(d.call("POST", "/node/stop-jobs", Some(&json!({}))));
            if json_out {
                return println!("{r:#}");
            }
            println!("cancelled {} job(s)", r["cancelled"]);
        }
        "coordinators" => match a.pos().as_deref() {
            None | Some("list") => {
                finish(a);
                let r = or_die(d.call("GET", "/node/coordinators", None));
                if json_out {
                    return println!("{r:#}");
                }
                let cs = r["coordinators"].as_array().cloned().unwrap_or_default();
                if cs.is_empty() {
                    println!("no coordinator is paired");
                }
                for c in cs {
                    println!("{:<24} {}", s(&c["name"]), s(&c["fingerprint"]));
                }
            }
            Some("remove") => {
                let name = a.pos().unwrap_or_else(|| {
                    die("which coordinator? `mason node coordinators remove NAME`")
                });
                finish(a);
                as_owner();
                let r = or_die(d.call(
                    "POST",
                    "/node/coordinators/remove",
                    Some(&json!({"name": name})),
                ));
                if json_out {
                    return println!("{r:#}");
                }
                println!("{name} is no longer trusted by this node");
            }
            Some(other) => die(&format!("unknown command {other:?}\n       {COMMANDS}")),
        },
        "list" => {
            finish(a);
            let r = or_die(d.call("GET", "/nodes", None));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "nodes are {} (prefer {})",
                if r["enabled"] == true { "on" } else { "off" },
                s(&r["prefer"])
            );
            let rows = r["nodes"].as_array().cloned().unwrap_or_default();
            if rows.is_empty() {
                println!("no node found or paired (`mason node add`)");
            }
            for n in rows {
                let paired = n["paired"] == true;
                let mut line = format!(
                    "{:<16} {:<22} {}",
                    s(&n["name"]),
                    format!("{}:{}", s(&n["host"]), n["port"]),
                    s(&n["state"])
                );
                if let Some(u) = n["update"].as_str().filter(|_| paired) {
                    line.push_str(&format!("  [runtime: {u}]"));
                }
                if paired && n["cpu_pct"].is_number() {
                    line.push_str(&format!(
                        "  CPU {}  die {}  jobs {}/{}  compile free {}",
                        pct(&n["cpu_pct"]),
                        n["thermal"]["die_c"]
                            .as_f64()
                            .map_or("?".into(), |c| format!("{c:.0} °C")),
                        n["jobs"]["running"],
                        n["jobs"]["max"],
                        n["slots"]["compile"]["free"],
                    ));
                }
                println!("{line}");
            }
        }
        "add" => {
            let host = a.opt("--host");
            let fp = a.opt("--fp");
            let code = a.opt("--code");
            let name = a.opt("--name");
            let target = if host.is_none() { a.pos() } else { None };
            finish(a);
            if host.is_none() && target.is_none() {
                die("which node? `mason node add NAME` (found over mDNS) or `mason node add --host H:PORT --fp FINGERPRINT`");
            }
            let code = code.unwrap_or_else(|| {
                eprint!("Pairing code (from `mason node pair-code` on the node): ");
                let mut line = String::new();
                let _ = std::io::stdin().read_line(&mut line);
                line.trim().to_string()
            });
            as_owner();
            let r = or_die(d.call(
                "POST",
                "/nodes/add",
                Some(
                    &json!({"target": target, "host": host, "fp": fp, "code": code, "name": name}),
                ),
            ));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "paired with {} ({}:{})",
                s(&r["paired"]),
                s(&r["host"]),
                r["port"]
            );
            println!("`mason node list` shows whether it is taking work; `mason node use on` lets runs go to it.");
        }
        "remove" => {
            let name = a
                .pos()
                .unwrap_or_else(|| die("which node? `mason node remove NAME`"));
            finish(a);
            as_owner();
            let r = or_die(d.call("POST", "/nodes/remove", Some(&json!({"name": name}))));
            if json_out {
                return println!("{r:#}");
            }
            println!("{} is no longer paired", s(&r["removed"]));
        }
        "update" => {
            let all = a.flag("--all");
            let name = if all { None } else { a.pos() };
            finish(a);
            if !all && name.is_none() {
                die("which node? `mason node update NAME` or `mason node update --all`");
            }
            as_owner();
            let r = or_die(d.call(
                "POST",
                "/nodes/update",
                Some(&json!({"name": name, "all": all})),
            ));
            if json_out {
                return println!("{r:#}");
            }
            println!("runtime {}", s(&r["runtime"]));
            for n in r["nodes"].as_array().into_iter().flatten() {
                println!(
                    "{:<16} {}: {}",
                    s(&n["name"]),
                    s(&n["state"]),
                    s(&n["note"])
                );
            }
        }
        "use" => {
            let on = on_off(a, "mason node use");
            finish(a);
            as_owner();
            let r = or_die(d.call("POST", "/nodes/use", Some(&json!({"on": on}))));
            if json_out {
                return println!("{r:#}");
            }
            println!(
                "[nodes] enabled = {} (written to daemon.toml; the next run uses it, no restart)",
                r["enabled"]
            );
        }
        other => die(&format!(
            "unknown node command {other:?}\n       {COMMANDS}"
        )),
    }
}

/// `mason node export-runtime --arch ARCH --out DIR`: the signed runtime, "MASON Daemon.app" and
/// the node user's `mason` wrapper for a node of another architecture, written into `DIR` to copy
/// to the node (docs/nodes.md). It
/// needs no daemon: it reads `update.toml` and the runtimes directory. A cross build is a build:
/// run it as `mason run --needs compile -- <runtime's mason> node export-runtime ...` to wait for
/// a slot (`mason run` execs a file, so a `mason` shell function will not do after `--`).
pub fn export_runtime(
    mut rest: Vec<String>,
    data_dir: &std::path::Path,
    config_dir: &std::path::Path,
) -> i32 {
    let mut opt = |name: &str| -> Option<String> {
        let i = rest.iter().position(|a| a == name)?;
        rest.remove(i);
        (i < rest.len()).then(|| rest.remove(i))
    };
    let (Some(arch), Some(out)) = (opt("--arch"), opt("--out")) else {
        eprintln!("error: mason node export-runtime --arch arm64|x86_64 --out DIR");
        return 1;
    };
    let cfg = match mason_daemon::update::UpdateCfg::load(config_dir) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return 1;
        }
    };
    let rt = mason_daemon::update::Runtimes::new(data_dir);
    let out = std::path::PathBuf::from(out);
    if let Err(e) = std::fs::create_dir_all(&out) {
        eprintln!("error: {}: {e}", out.display());
        return 1;
    }
    match mason_daemon::node_runtime::export(&cfg, &rt, &arch, &out) {
        Ok(sha) => {
            println!(
                "wrote the signed runtime {} for {arch} to {}/runtimes/{sha}, and MASON Daemon.app and the mason wrapper to {}/",
                &sha[..sha.len().min(10)],
                out.display(),
                out.display()
            );
            println!("Copy the folder to the node and install it as docs/nodes.md says.");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}
