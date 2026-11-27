# 1167: a failed task leads with its diagnosis; every card takes context

Decision: the page choice (card, diagnosing, buttons) and the answer request are plain logic in MonitorCore and Compose core, tested there; the daemon sends `diagnosis` on rows and `diagnosis_running` on the task view, and the WebSocket `Answer` carries an optional `context`.
Why: the owner wants the diagnosis, not Retry, as the first thing on a failure, and to add words to any answer.
Retry, Cancel and Skip all testing fold under "Other actions" only while a card or a running diagnosis is shown; with neither, the old buttons stay as they were.
`clients/features.toml` is protected: the two new entries wait for the owner's approval.
