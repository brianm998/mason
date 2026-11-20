# 0654: Task references in the Swift monitor are links, checked by a source scan

Status: accepted (stage 661)

Every string that can name a task is shown through `LinkedText`. Rows that were one Button (home stage rows, the phone list) become a tap gesture on the row, because a link inside a button's label cannot be clicked on its own. The sidebar row stays a button with a plain wait line: it is lazy, compared by value and has no model, and the task page links the same text. A unit test scans `Sources/MonitorUI/*.swift` for `Text(` that interpolates `#\(` or passes wait, batch, lease or hold strings, and fails unless the line is on a short allowlist with a reason, so the habit cannot return silently.
