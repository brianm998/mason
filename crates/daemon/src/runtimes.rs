// SPDX-License-Identifier: GPL-3.0-or-later
//! How long a `mason run` command takes (s400): what the scheduler's backfill estimates from,
//! so a drain held by an agent's own release build can still let short work pass.
//!
//! Times are kept in the store's `suite_times` table, with the normalized command line in the
//! `suite` column and phase [`PHASE`].

/// The `suite_times` phase of a `mason run` command.
pub const PHASE: &str = "run-cmd";

/// The command line with what changes between runs taken out: the working directory's path
/// becomes `<w>` and every run of digits `#`, so `cargo build -j 8` in two worktrees is one
/// command.
pub fn normalize(cmd: &[String], cwd: &str) -> String {
    let joined = cmd.join(" ");
    let cwd = cwd.trim_end_matches('/');
    let text = if cwd.len() > 1 {
        joined.replace(cwd, "<w>")
    } else {
        joined
    };
    let mut out = String::with_capacity(text.len());
    let mut in_digits = false;
    for ch in text.chars() {
        if ch.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn worktree_paths_and_numbers_are_taken_out() {
        let a = normalize(&c(&["cargo", "build", "-j", "8", "--manifest-path", "/w/t412/Cargo.toml"]), "/w/t412");
        let b = normalize(&c(&["cargo", "build", "-j", "16", "--manifest-path", "/w/t977/Cargo.toml"]), "/w/t977");
        assert_eq!(a, b);
        assert_eq!(a, "cargo build -j # --manifest-path <w>/Cargo.toml");
        assert_ne!(a, normalize(&c(&["cargo", "test"]), "/w/t412"));
    }
}
