# 0265: paused banners carry their project and mode; collapse is a per-device preference

**Decision.** `Banner` carries `project` and `mode` for `paused` banners so a monitor can send `ResumeWork` for exactly that pause. Collapsing a pause banner is stored per device under `paused:<project>:<mode>` (with an index key for pruning) and dropped when the pause leaves the header.

**Why.** Pauses pile up as full-width rows that cannot be shrunk or acted on. A client cannot rebuild the pause's identity from the banner text, and `resume()` with an empty mode lifts every mode of the project, which is too broad for one banner's button.

**Trust.** `clients/features.toml` is in `PROTECTED` (client parity): the new `paused-collapse` entry, done on both clients, needs the owner's approval. Nothing else touched is protected; no gate, limit or test is weakened.
