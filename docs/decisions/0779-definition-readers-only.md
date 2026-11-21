# 0779: `.mason/` files are read only by targets naming the repository copy

A target reads the repository's `.mason/<file>` when its text names `../../<path>` or `root().join("<path>")`. A fixture that writes its own `.mason/mason.toml` does not, so the bare-name match is dropped for `.mason/` paths. Other paths keep the old rule. Known gap: src-level unit tests are not scanned.
