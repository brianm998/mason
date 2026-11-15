# 0423: Settings page edits the config files in place, through the loaders' parsers

Decision: the Settings page does not keep its own copy of any setting. Each change is applied to the file text with `toml_edit`, checked by the parser that loads that file (`config::parse_project`, `notify::parse`), written atomically, and applied with the existing reload (`reload_registration`; `notify.toml` is read every tick). A refused reload puts the old text back. Only a fixed list of keys is editable, and only the owner may write.

Why: the files stay the source of truth and keep working when edited by hand, comments survive, and a value the page accepts is by construction one the loader accepts.

Secrets: the page never shows or accepts a secret. The tracker's `token` and `webhook_secret` can only be set to an `env:`/`file:` reference; the email, ntfy and webhook sections are not editable at all. A literal secret already in a file is reported as "set in file, not shown".

Rejected: a database copy of the settings (two sources of truth, and a hand edit would be silently overridden); editing the speech command (it runs a program); an owner check on `/resources/limit` in this task (monitors' `SetLimit` may send no identity; separate decision).

Not verified: see `docs/stages/s430.md`.
