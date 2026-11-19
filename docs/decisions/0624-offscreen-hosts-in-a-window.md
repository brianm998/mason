# 0624: offscreen test hosts live in a never-ordered window

Decision: RowRedrawTests, FollowOnlyWhatTheyShowTests and LazySidebarTests host their
NSHostingView as the contentView of an NSWindow that is never ordered in
(`defer: true`, `isReleasedWhenClosed = false`), kept alive by the test case.

Why: on the CI runner a window-less host evaluated zero bodies (the log showed
`s237: 0 RowView body evaluations`), so the `before > 0` assertions failed.

Not changed: frames, fixtures, settle durations and every assertion. Nothing is shown on screen.
