# 0530: cache image, link and voice work outside `body`

Decision: the mascot images, linkified text segments and the OS voice list are cached where they are produced (`Mascot`, `MonitorModel`, `Announcer`/`SystemSink`), not in each view.

Why: every view that calls them from `body` benefits, including iOS, and the cached values are either immutable (bundle art) or keyed to a revision counter that the model already bumps on change (sidebar, open task). Caches never need manual invalidation beyond that. The link cache is capped at 512 texts.

Not done: loading voices off the main thread; the cache makes it a single call.
