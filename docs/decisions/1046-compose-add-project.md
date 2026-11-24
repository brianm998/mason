# 1046: Add Project in the Compose monitor is desktop-only state in core, drawn by app

The wizard's state (answers, step, last plan, landing and proof choices) is in `core` so it is unit tested without a window and shares its naming with the Swift client. The dialog, menu item and + live in `app`, which the Android app does not use; `MonitorModel.addProjectAvailable` is also false on Android, and a core test fails if Android sources mention Add Project.

Landing offers no, merge, push and pr, default no, and is always sent in `answers` so the daemon never asks it as a question. The proof task is on by default (`no_prove` is sent only when it is turned off). The folder picker is offered only when the daemon was found through its own daemon.json, since a path chosen on this machine means nothing to a remote daemon.
