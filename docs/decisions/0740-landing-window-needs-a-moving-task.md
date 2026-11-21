# 0740: the landing window is waited out only for a task that is moving

Status: accepted (#731)

The window collects tasks that arrive close together. With no other task on its way to landing it only delays the batch. `form_batch` therefore gets the count of tasks still moving and forms at once when it is zero.

Moving means: exploring, developing, testing, review or landing but not yet in the queue or a batch, or queued with no blocker; not paused. Held, failed, proposed, deferred, cancelled and needs-attention tasks do not count, so a task blocked behind a waiting one never holds it.

Detecting a task that stops moving is done by looking again within a second while a batch waits, not by waking on every task transition. That is a cheap world read; an event hook in the store would touch every writer.

This changes when a batch forms, not what is checked or how it is judged (landing rules in `PROTECTED`: `crates/landing/**`, `crates/daemon/src/landing.rs`; owner approval applies).
