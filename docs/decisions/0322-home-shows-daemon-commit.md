# 0322: Home shows the daemon's and the monitor's commit

Decision: home draws one line, `masond <8 chars> · monitor <8 chars>`, from the welcome's
`version` and the monitor's `ownVersion`. Full hashes are in the tooltip.

Why parse the sentence: `UpdateInfo` carries only text for the daemon's lag. Adding a field
would change the protocol for a label; parsing "N commits behind" keeps this client-only.
If the daemon's wording changes the count disappears (the hash still shows).

Consequence: `clients/features.toml` (protected) gains a row; owner approval needed.
