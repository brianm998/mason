// SPDX-License-Identifier: GPL-3.0-or-later

//! The line `mason --version` and `masond --version` print.

/// `mason <version> (<commit>)`; `unknown` when the commit is empty.
pub fn line(version: &str, commit: &str) -> String {
    let commit = if commit.is_empty() { "unknown" } else { commit };
    format!("mason {version} ({commit})")
}

#[cfg(test)]
mod tests {
    use super::line;

    #[test]
    fn the_line_has_version_and_commit() {
        assert_eq!(line("0.1.0", "abc123"), "mason 0.1.0 (abc123)");
        assert_eq!(line("0.1.0", ""), "mason 0.1.0 (unknown)");
    }
}
