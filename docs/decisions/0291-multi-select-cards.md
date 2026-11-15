# 0291: a decision card can take several answers

**Decision.** A card has an optional `multi_select` flag. On such a card the answer is a list of option numbers ("1,3"); the daemon resolves it to the labels in card order, joined with "; ", and that text is what the Answered event stores and the agent reads. "Other: text" stays a single answer.

**Why.** One question with several answers had to be asked as several questions. A flag on the card, not a new answer type, keeps the protocol's Answer message (one string) and every stored answer as they were. This replaces the "not built: multi-select" note in decision 0018.

**Compatibility.** `to_json` writes the flag only when true, so existing cards and their hashes do not change. `DecisionCard.multi_select = 7` is an additive protocol field.

**Trust.** `contract.rs` (the ask schema), `clients/features.toml` and the new `scripts/exit/s297.sh` are protected and wait for the owner. Nothing is weakened.
