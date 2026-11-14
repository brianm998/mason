# 0293: the editor is tested offscreen through its actions; real checks are listed in one place

**Decision.** The editor's control actions live in `EditorActions` (Swift) so a test calls what a button calls; both clients test the editor offscreen (a never-shown hosting view, Compose's test rule). The checks that need a real service, token or device are listed in `docs/real-checks.md`, each script ends with a `RESULT` line, and `s299.sh` fails when a `*-real.sh` is not listed.

**Why.** Features built against fakes counted as done with nothing to say when a real run was owed. A SwiftUI `Button` cannot be tapped without a new dependency; moving the action out keeps the closure the button runs the one the test runs, with no new dependency. The owner may prefer ViewInspector later.

**Trust.** `scripts/exit/**` and `.github/workflows/**` are protected: `s299.sh`, `s20-real.sh`, the RESULT lines and the CI jobs await the owner's approval. Nothing is weakened.
