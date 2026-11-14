# 0203: Update now while an update waits for a landing batch

Status: accepted, pending the owner's approval of the protected files.

**Decision.** The daemon words the waiting banner, the button and the confirmation, and the monitors only show them. `POST /update/now` sets a flag the drain loop checks; it is the only way to stop a wait before `takeover_wait_s`. `POST /update` with `now` does the same while an update waits.

**Why.** Pressing "Update masond" while an update waited gave a 409 nobody saw. Daemon-written text keeps both monitors identical; a flag (not a second takeover path) reuses the drain's abandon logging and the restart requeue.

**Not done.** Per-batch CI state in the sentence waits for #261.
