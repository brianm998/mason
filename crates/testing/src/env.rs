// SPDX-License-Identifier: GPL-3.0-or-later

//! An environment's lifecycle as commands: lease, setup, health check,
//! teardown (Part 8). The daemon runs them; this says which, in what order.
//! Simulators, emulators and devices (Stage 25) fill in the same four phases,
//! so nothing here is specific to a local machine or Docker.

use crate::model::{EnvKind, Environment};

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
        EnvKind::Local => {
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
}
