# 0953: The Swift monitor holds its views while the window is hidden

Stage s960. Each daemon push assigned a new value to the `@Observable` model even with the window hidden, so SwiftUI re-ran its graph update for nothing (5:39 of main-thread CPU in 100 hidden minutes).

Decision: one `visible` flag on the model, driven from the same source as `SharedTick`. Hidden: header and sidebar go to held, unobserved slots (logic reads them through `liveSidebar`), progress is kept as the newest message per task and step, and the subscription is header and sidebar only. Visible again: publish once, flush progress once, resubscribe; the daemon resends the page's views. Rejected: dropping the connection while hidden, which would lose the attention chime and command settlement.

Not verified: see docs/stages/s960.md.
