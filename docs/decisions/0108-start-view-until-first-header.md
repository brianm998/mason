# 0108: The start view is gated on the first header

The start view shows while `model.header == nil`. The header is set on the first header view and never cleared, so no extra "has connected" flag is needed and a later disconnect cannot bring the start view back; the existing banners show it instead. The status text comes only from `connected` and `reconnecting`, so it is always the real state. No progress bar and no minimum time: the design forbids estimated progress.

The image is fitted whole because the robot (left) and the word MASON (right) would both be lost by a centred crop. A native iOS launch screen needs an Xcode project and is out of scope.

`clients/features.toml` is a protected path; this change only adds a feature entry and weakens no check. The new token was added by hand (see the stage note).
