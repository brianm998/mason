# 1297: The projsettings failures: fixtures, one expectation and the provider header

**Context.** Two `projsettings` tests failed on `3c8b855462`.

**An unknown adapter is refused.** The provider test set `provider = "codex"` with no `providers.toml`, so the only account was the default claude one. `settings::set_project_as` writes the file, `config::load_project` finds no account for the codex adapter (`providers::resolve_account`), and the old text is put back with a 400. That is correct: a registration masond cannot load must not stay on disk. The test now provides a codex account.

**A key's comments go with it.** The comment test unsets `tool_timeout_s`, and the fixture's only two comments were above it and at the end of its line. `projsettings::unset_at` removes the key, and `toml_edit` removes the comments attached to it. That is correct: a comment about a key says nothing once the key is gone. The fixture's comments now sit above the `[proposals]` header, which no sampled edit removes.

**A required key stays.** The loop expected unsetting `test` to succeed. `config::parse_project` requires it, so the write is refused with a 400 and the file is left as it was. The test now asserts that.

**The header was wrong.** Turning `provider = "codex"` into a table kept the key's spacing, so the file read `[provider ]`, and the comments on that line were dropped. The writer now clears the spacing and moves the comments to the header and to `use`.

**Rejected.** Making `unset_at` move a removed key's comments to the next item. It would leave notes describing a setting that is no longer there, and could attach them to an unrelated key.
