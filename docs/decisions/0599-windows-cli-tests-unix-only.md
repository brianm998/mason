# 0599: gate the unix-only CLI test module on unix

Decision: `open_session_tests` is `#[cfg(all(test, unix))]` rather than ported to Windows. Its fake provider is a shell script with unix mode bits; the tests still run on every unix runner.
