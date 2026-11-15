# 0396: build failures are named by location, not by their text

**Decision.** In the regex parser, a failing line is named by `path:line` when it is a compiler diagnostic, else by its capture, else by the fixed name `build`.

**Why.** Naming by the whole line put Swift source lines into flake records and quarantine (57 sightings in four days).

**Trust.** `crates/testing/**` is in `PROTECTED`; the change is needed there because `regex_lines` is the naming point. Counts and verdicts are unchanged.
