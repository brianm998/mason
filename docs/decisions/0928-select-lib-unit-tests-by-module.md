# 0928: Unit tests of a changed src file are selected by module

Stage s935. Any change under `src/` ran the package's `--lib --bins` whole, so a mason-daemon change never ran fewer than about 632 tests.

Decision: for a `src/` change, run the changed module's unit tests and those of each module whose text names it, as libtest filters (`-p mason-daemon --lib --bins -- covmap:: affected::`), in a group of its own because cargo applies a filter to every target of a command. The scan is the same kind of name scan `references` does for integration targets. The lib runs whole when the file is a crate root, a declaration-only `mod.rs`, `build.rs` or carries `macro_export`, when a root or binary with unit tests names the module, or when more than half the lib's modules are picked.

Only the per-task step narrows. The landing batch's tier and the scheduled full test run everything, so nothing reaches `develop` less tested. A later coverage map for the lib would replace the name scan.
