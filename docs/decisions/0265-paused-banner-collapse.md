# 0265: paused banners carry their project and mode; collapse is a per-device preference

**Decision.** `Banner` carries `project` and `mode` for `paused` banners so a monitor can send `ResumeWork` for exactly that pause. Collapsing a pause banner is stored per device under `paused:<project>:<mode>` (with an index key for pruning) and dropped when the pause leaves the header.

**Why.** Pauses pile up as full-width rows that cannot be shrunk or acted on. A client cannot rebuild the pause's identity from the banner text, and `resume()` with an empty mode lifts every mode of the project, which is too broad for one banner's button.

**Trust.** `clients/features.toml` is in `PROTECTED` (client parity): the new `paused-collapse` entry, done on both clients, needs the owner's approval. `crates/daemon/src/suites.rs` is protected (verdict logic: running suites): #282's last-round base runs now get the suite's device and environment, because without them a device suite's tests ran at the base with no device and the check could not mean anything. That needs the owner's approval too. No gate, limit or test is weakened: the e2e and landing tests were changed to match behaviour develop already has (#282's base runs; the base-red excuse of decision 0241), and each still checks that nothing red lands.
