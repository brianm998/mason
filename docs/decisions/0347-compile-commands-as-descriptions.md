# 0347: long interactive commands are shown as a short description that opens a dialog

**Decision.** The daemon sends an interactive command's one-line description (at most 49 characters plus an ellipsis, no newline) as `label`, and the whole command in a new `command` field on `CompileHolder` and `CompileWaiter` (and in the `/resources` JSON). A waiting reason (`behind <description> in line`) embeds the description only. One function, `mason_scheduler::describe_command`, builds it; the waiting-command sentence uses it with its old 40-character limit. Both monitors show the description as a link that opens a dialog holding the command (monospaced, selectable, Copy, Close, Escape closes); the dialog holds a copy, so it survives the row leaving the queue.

**Why.** A pasted script printed whole filled the Compilation page. Hover text cannot be copied and the client cannot cut a string with the script embedded in it.

**Notes.** The `interactive ` prefix is gone from the monitor label (the row already says "interactive"). A reason is one line but longer than 50 characters because of its fixed words; the label and the description inside it are at most 50. The field is new and defaulted, so old clients and old payloads work.

**Trust.** No `PROTECTED` file changed.
