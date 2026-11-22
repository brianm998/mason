# 0864: `{file}` and `{path}` placeholders in `narrow_each`

Status: accepted (owner asked 2026-11-21, task #924).

Context: a narrowed unit was named only by its dotted name, which suits JVM classes but not pytest (star #888 failed three times with "file or directory not found: tests.test_engine").

Decision: add `{file}` (path under `narrow_root`) and `{path}` (from the repo root); keep `{}`. `definition check` warns, not refuses, for pytest with `{}` alone, since a custom runner could still take dotted names.

Consequence: the selected tests are unchanged; only how they are named to the runner changes. Existing suites behave as before.
