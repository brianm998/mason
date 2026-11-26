// SPDX-License-Identifier: GPL-3.0-or-later

//! How the daemon formats its log. Colour codes belong on a terminal; in the
//! log file launchd (or a test) points stderr at, they split `task=1` into
//! `ESC[3mtask ESC[0m ESC[2m= ESC[0m1` and no `grep` for a message matches.

use std::ffi::OsStr;

/// Pure: whether log lines may carry ANSI colour. Only when stderr is a
/// terminal and `NO_COLOR` is unset (see no-color.org: an empty value counts as
/// unset).
pub fn log_ansi(stderr_is_terminal: bool, no_color: Option<&OsStr>) -> bool {
    stderr_is_terminal && no_color.is_none_or(|v| v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_only_on_a_terminal_without_no_color() {
        assert!(log_ansi(true, None));
        assert!(!log_ansi(false, None));
        assert!(!log_ansi(true, Some(OsStr::new("1"))));
        assert!(!log_ansi(false, Some(OsStr::new("1"))));
    }

    #[test]
    fn an_empty_no_color_is_unset() {
        assert!(log_ansi(true, Some(OsStr::new(""))));
    }
}
