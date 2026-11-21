# 0764: Row-redraw counts without a window

Status: accepted

The row-redraw tests need SwiftUI to evaluate bodies. An unordered NSWindow with a layout and
display cycle did not do so on the GitHub runner. ImageRenderer renders synchronously with no
window and no screen, and the repo's other offscreen tests already use it. We use it and keep the
strict `> 0` preconditions; a runner that evaluates nothing stays red rather than skipping.
