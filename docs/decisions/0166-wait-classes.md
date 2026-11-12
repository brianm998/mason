# 0166: Waits are totalled by class, set where the wait starts

The digest added up wait time under the full text, which holds the queue position and holder, so every line was unique. 

**An optional field, not a new event.** `WaitStarted` gets `class: Option<String>`, stored as a trailing codec field like the other late additions. Old logs and golden fixtures decode with `None` and replay the same; a new log written by this build and read by an older one would fail on the extra field, which the project accepts for every trailing field.

**Class from the request.** The scheduler's own `Wait::class()` names the resource (`compile`, `test-suite`, `llm:claude`, `device:...`) or the gate (`cpu` for the CPU gate and settling, `disk-stall`, `disk-floor`, `memory`, `exclusive`). A grant held up by a kernel lock something outside the queue holds is `outside-pipeline`. The broker keeps the class of the wait being announced in a map for the length of the `on_wait` call, so the callback signature (and every caller, test included) is unchanged. Pause is `pause`; approval is `approval`; git and account-hold and suite environment waits are `environment`. A device pool with no units is classed by its `device:<kind>` resource.

**Old events.** `wait_class(text, None)` reads the prefix of the text; anything unknown is `other`. The derived class for a text and the class set at the start of a new wait agree for the texts the scheduler writes today.

**Counting.** The digest replay compares the world before and after each event and counts a wait when it leaves (ended, or replaced), and any wait still open when the window ends. Duration is clipped to the window, so a class's total equals the sum of its waits' durations. Queue and owner time are charged as before (they feed `wait_share`) under `queue` and `owner`, with no count.

**Not done.** No fixture regenerated. No change to `wait_share` or any threshold.
