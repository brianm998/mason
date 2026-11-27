# 1229: onboard uses the platform fs helpers

Onboard calls `mason_platform::fs` for the execute-bit check and for setting it, instead of inline `cfg` blocks, as the other crates do. `is_file()` stays in onboard because the platform Unix branch tests only the mode bits.
