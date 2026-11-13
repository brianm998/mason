// SPDX-License-Identifier: GPL-3.0-or-later

//! This machine's name.

/// The host name, or `None` if the system will not say.
pub fn name() -> Option<String> {
    #[cfg(unix)]
    {
        nix::unistd::gethostname().ok()?.into_string().ok()
    }
    #[cfg(windows)]
    {
        // The NetBIOS computer name, which Windows sets in the environment.
        std::env::var("COMPUTERNAME").ok()
    }
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn a_unix_host_has_a_name() {
        assert!(super::name().is_some_and(|n| !n.is_empty()));
    }
}
