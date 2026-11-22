# 0790: retry a transient push rejection and wait out a GitHub outage in landing

Status: proposed (landing.rs is a guard file; owner approval needed)

Decision: `remote::push` retries remote-side failures three times with backoff; landing runs every remote check through one helper that waits (30 s doubling to 5 min, 30 min total) on an outage error and only then raises the service failure; those failure reasons classify as `outside`.

Why: on 2026-11-20 a single GitHub rejection failed #746's landing and charged a `[code]` attempt, though the next push worked minutes later.

Not verified: behaviour against real GitHub; the test step and debt payments are not covered (see s797).
