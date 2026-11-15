# 0316: per-task landing status is built in the view layer

Decision: `views.rs` joins live batches, the queue accessor, freezes and the landing config into `LandingView.items`, instead of adding an accessor to `landing.rs` (a `PROTECTED` file).

Consequence: time in phase is time since the batch formed, and the window close time is not shown; both need a `landing.rs` accessor, which waits for the owner.
