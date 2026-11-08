// SPDX-License-Identifier: GPL-3.0-or-later

//! An environment's lifecycle as commands: lease, setup, health check,
//! teardown (Part 8). The daemon runs them; this says which, in what order.
//! Devices (Stage 25) fill in the same four phases: the provider readies the
//! device and these commands run on top of it, so nothing here is specific to
//! a local machine, Docker or a phone.

use crate::model::{DeviceType, EnvKind, Environment};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Phase {
    pub name: &'static str,
    pub command: String,
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Run these in order to bring the environment up. A failing phase means the
/// environment is not usable, which is the environment's fault, not the
/// task's.
pub fn up(e: &Environment) -> Vec<Phase> {
    let mut v = vec![];
    match e.kind {
        // A device is readied by its provider; these are the project's own
        // commands on top, run with MASON_DEVICE set.
        EnvKind::Local | EnvKind::Device(_) => {
            if let Some(s) = &e.setup {
                v.push(Phase {
                    name: "setup",
                    command: s.clone(),
                });
            }
        }
        EnvKind::Compose => {
            if let Some(s) = &e.setup {
                v.push(Phase {
                    name: "setup",
                    command: s.clone(),
                });
            }
            let file = e.file.as_deref().unwrap_or("docker-compose.yml");
            v.push(Phase {
                name: "up",
                command: format!("docker compose -f {} up -d --wait", quote(file)),
            });
        }
    }
    v
}

/// The health check, retried by the caller until it passes or
/// `health_timeout` is spent.
pub fn health(e: &Environment) -> Option<Phase> {
    e.health.as_ref().map(|h| Phase {
        name: "health",
        command: h.clone(),
    })
}

/// Always run, whatever happened, once the environment was brought up (or an
/// attempt was made to).
pub fn down(e: &Environment) -> Vec<Phase> {
    let mut v = vec![];
    if e.kind == EnvKind::Compose {
        let file = e.file.as_deref().unwrap_or("docker-compose.yml");
        v.push(Phase {
            name: "down",
            command: format!("docker compose -f {} down -v", quote(file)),
        });
    }
    if let Some(t) = &e.teardown {
        v.push(Phase {
            name: "teardown",
            command: t.clone(),
        });
    }
    v
}

/// The shell prefix that tells a command which device it is on. The id is quoted.
pub fn with_device(command: &str, device: Option<&str>) -> String {
    match device {
        Some(id) => format!("export MASON_DEVICE={}; {command}", quote(id)),
        None => command.to_string(),
    }
}

/// The device type an environment leases, if it is a device environment.
pub fn device_of(e: &Environment) -> Option<DeviceType> {
    match e.kind {
        EnvKind::Device(t) => Some(t),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_local_environment_is_just_its_own_commands() {
        let mut e = Environment::local("db");
        assert!(up(&e).is_empty() && down(&e).is_empty() && health(&e).is_none());
        e.setup = Some("make db".into());
        e.health = Some("pg_isready".into());
        e.teardown = Some("make db-stop".into());
        assert_eq!(up(&e)[0].command, "make db");
        assert_eq!(health(&e).unwrap().command, "pg_isready");
        assert_eq!(down(&e)[0].command, "make db-stop");
    }

    #[test]
    fn a_compose_stack_comes_up_waiting_and_always_goes_down() {
        let mut e = Environment::local("stack");
        e.kind = EnvKind::Compose;
        e.file = Some("ci/it's.yml".into());
        e.setup = Some("./seed.sh".into());
        e.teardown = Some("./cleanup.sh".into());
        let u = up(&e);
        assert_eq!(u[0].command, "./seed.sh");
        assert_eq!(
            u[1].command,
            "docker compose -f 'ci/it'\\''s.yml' up -d --wait"
        );
        let d = down(&e);
        assert_eq!(d[0].command, "docker compose -f 'ci/it'\\''s.yml' down -v");
        assert_eq!(d[1].command, "./cleanup.sh");
    }

    #[test]
    fn a_device_environment_runs_the_projects_commands_with_the_device_named() {
        let mut e = Environment::local("phone");
        e.kind = EnvKind::Device(DeviceType::IosDevice);
        e.setup = Some("./install.sh".into());
        assert_eq!(up(&e)[0].command, "./install.sh");
        assert_eq!(device_of(&e), Some(DeviceType::IosDevice));
        assert_eq!(
            with_device("./install.sh", Some("ab'c")),
            "export MASON_DEVICE='ab'\\''c'; ./install.sh"
        );
        assert_eq!(with_device("x", None), "x");
    }
}
