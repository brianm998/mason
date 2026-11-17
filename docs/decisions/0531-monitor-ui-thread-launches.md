# 0531: Compose monitor process launches run on an injected background executor

Status: accepted (task #521).

Context: Open session waited up to 2 s on the UI thread, the voice picker ran `say -v ?` on every recomposition, and linkify, known-task scans and preference reads repeated per frame.

Decision: the model carries `background` and `ui` executors (inline by default so tests and headless use are deterministic). Launches run on `background`; failures hop to `ui`. Voices load once into announcer state. Known ids, linkified segments and section flags are cached and invalidated by value identity of the sidebar/task page.

Consequences: `launchSession` is asynchronous, so the copied-command notice appears after the launcher returns. Section flags changed in the store by another process are not seen until restart.
