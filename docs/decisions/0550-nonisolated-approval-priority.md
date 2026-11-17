# 0550: mark the approval priority constants nonisolated

Default-argument expressions are nonisolated, so a `@MainActor` static read there warns (an error in Swift 6 mode). The constants are immutable `String` values, so `nonisolated` is safe. We kept the `"normal"` default and the Swift 5 language mode, and did not use `@preconcurrency`.
