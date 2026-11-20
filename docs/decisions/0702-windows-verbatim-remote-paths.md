# 0702: strip the verbatim prefix in the test fixture, not in fakehost or with a new crate

Context: CI/windows was red on develop because the github adapter test passed a `\\?\C:\...` path (from `canonicalize`) to `git clone`, which parsed it as `host:path`.

Decision: `fake_world` in crates/adapters/tests/github.rs uses a small helper that strips `\\?\` before a drive letter. We did not edit fakehost.rs (it receives the bare path) and did not add the `dunce` crate.

Consequences: only the test fixture changes; production git paths are untouched and Unix behaviour is identical. Verification is the next CI/windows run.
