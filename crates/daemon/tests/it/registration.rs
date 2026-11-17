// SPDX-License-Identifier: GPL-3.0-or-later

//! Registering a project while masond runs (`mason init` does this, so its first task needs no
//! restart).

use crate::support;
use support::*;

#[test]
fn a_new_project_is_registered_without_a_restart() {
    let s = Scratch::new("true", happy_script());
    let d = s.start();
    assert!(d.project_id_by_name("second").is_none());

    // No file, or a file that does not load: refused, and nothing changes.
    assert!(d.register_project("second").is_err());
    std::fs::write(s.config_dir.join("projects/second.toml"), "= = nope").unwrap();
    assert!(d.register_project("second").is_err());
    assert!(d.project_id_by_name("second").is_none());

    s.write_project("second", "true", "");
    let (pid, new) = d.register_project("second").unwrap();
    assert!(new);
    assert_eq!(d.project_id_by_name("second"), Some(pid));
    assert!(d
        .world()
        .unwrap()
        .projects
        .values()
        .any(|p| p.name == "second"));
    let _ = d.current_def(pid);

    // Registering again answers with the same id and changes nothing.
    let (again, new) = d.register_project("second").unwrap();
    assert_eq!(again, pid);
    assert!(!new);
}
