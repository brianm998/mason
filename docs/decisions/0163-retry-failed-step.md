# 0163: A retry resumes at the failed step when the failure was not about the code

Stage 168. A failure is classified once, when it is recorded, and the class is stored with it (`StepFailed.class`, `Task.failure_class`), instead of matching the failure text when the owner retries.

**Rule.** `Outside` (a landing "could not land" verdict, a lost host): a retry with no step names the failed step. `Code` (everything else): the first developing step, as before. No class (an event recorded earlier): the old text rule, which keeps only a lost command on its step.

**Why a stored class and not text at retry.** The text rule had to grow with every message. The place that ends the step knows which kind it is; the class travels with the event and replays the same.

**Why landing's `Failed` verdict is all `Outside`.** It is what landing returns when retrying inside the batch will not help and nothing was judged about the change: push, fetch, auth, services, CI service error. A red suite, a weakening and a conflict are `SentBack` or `Conflict` verdicts, not failures.

**Event format.** The class is an optional trailing field, so the log reads old events unchanged. I did not check what an older daemon does with a new event.

**Protected files.** None changed. The monitors' button label needs a `clients/features.toml` entry and waits for the owner.
