# Security policy

MASON runs coding agents with write access on a person's machine while reading
untrusted text, so security reports are taken seriously.

## Reporting

Do not open a public issue. Use GitHub's private vulnerability reporting
("Security" tab, "Report a vulnerability") on this repository. Include steps to
reproduce and the version or commit. The owner will acknowledge the report and
agree a disclosure timeline with you.

## Scope

The threat model is Part 22 of [docs/DESIGN.md](docs/DESIGN.md): prompt
injection, malicious PRs, secret leakage, dependency compromise and tampered
releases.

## Supported versions

Pre-1.0: only the latest `main`.
