// SPDX-License-Identifier: GPL-3.0-or-later

//! Is a red CI job infrastructure (a lost runner, a dead network, a full disk) or the
//! change's own failure? Pure: a conclusion and a log in, a reason out. The patterns are
//! starpipe's `INFRA_LOG` plus the shutdown-signal and name-resolution phrases GitHub's
//! hosted runners print; a project adds its own with `[ci] infra_patterns`.

/// Lower-case substrings; a log is matched case-insensitively.
pub const DEFAULT_PATTERNS: &[&str] = &[
    "runner lost",
    "runner has lost",
    "lost communication",
    "the runner has received a shutdown signal",
    "could not resolve host",
    "temporary failure in name resolution",
    "temporary failure resolving",
    "econnreset",
    "connection reset",
    "actions/cache",
    "failed to download",
    "curl: (",
    "no space left on device",
    "the operation was canceled",
    "could not resolve all dependencies",
    "could not resolve all files for configuration",
    "could not get resource",
    "could not transfer artifact",
    "failed to read artifact descriptor",
    "read timed out",
    "connect timed out",
    "received status code 500",
    "received status code 502",
    "received status code 503",
    "received status code 504",
    "502 bad gateway",
    "503 service unavailable",
    "504 gateway timeout",
];

/// Why a failed job looks like infrastructure, or `None` when it is a real failure.
/// `conclusion` is the job's (`failure`, `cancelled`, `timed_out`, `startup_failure`);
/// `steps_run` is whether any of its steps ran; `log` is its failed log, when fetched.
pub fn judge(
    conclusion: &str,
    steps_run: bool,
    log: Option<&str>,
    extra: &[String],
) -> Option<String> {
    match conclusion {
        "timed_out" => return Some("timed out".into()),
        "startup_failure" => return Some("failed to start".into()),
        "cancelled" if !steps_run => return Some("never got a runner".into()),
        "cancelled" => return Some("cancelled".into()),
        _ => {}
    }
    let log = log?.to_lowercase();
    DEFAULT_PATTERNS
        .iter()
        .map(|p| p.to_string())
        .chain(extra.iter().map(|p| p.to_lowercase()))
        .find(|p| !p.is_empty() && log.contains(p.as_str()))
        .map(|p| format!("runner or network trouble ({p})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn j(c: &str, log: &str) -> Option<String> {
        judge(c, true, Some(log), &[])
    }

    #[test]
    fn conclusions_decide_before_any_log() {
        assert!(j("timed_out", "").is_some());
        assert!(j("startup_failure", "").is_some());
        assert!(j("cancelled", "").is_some());
        assert_eq!(
            judge("cancelled", false, None, &[]).as_deref(),
            Some("never got a runner")
        );
    }

    #[test]
    fn infrastructure_phrases_are_infrastructure() {
        for l in [
            "##[error]The runner has received a shutdown signal.",
            "ping: Temporary failure resolving 'github.com'",
            "The self-hosted runner: Runner lost communication with the server",
            "curl: (56) Recv failure: Connection reset by peer",
            "Error: No space left on device",
            "Failed to download action",
            "Received status code 503 from server",
            "Could not resolve all dependencies for configuration ':app'",
            "Warning: actions/cache: Cache service responded with 500",
        ] {
            assert!(j("failure", l).is_some(), "{l}");
        }
    }

    #[test]
    fn code_failures_are_real() {
        for l in [
            "error[E0308]: mismatched types\n --> src/main.rs:3:5",
            "XCTAssertEqual failed: (\"1\") is not equal to (\"2\") - FooTests.testBar",
            "Test Case '-[FooTests testBar]' failed (0.002 seconds).",
            "error: cannot find 'x' in scope",
        ] {
            assert_eq!(j("failure", l), None, "{l}");
        }
        assert_eq!(judge("failure", true, None, &[]), None);
    }

    #[test]
    fn a_project_pattern_extends_the_defaults() {
        let extra = vec!["Simulator Flake".to_string()];
        assert!(judge("failure", true, Some("the simulator flake again"), &extra).is_some());
        assert!(judge("failure", true, Some("the simulator flake again"), &[]).is_none());
    }
}
