# 0264: compile rows name the task's project and title

**Decision.** The daemon adds `project` and `title` to each `CompileHolder` in the home view, looked up in the store by task id; the clients cut the title to 40 characters.

**Why.** With several projects queued, `#294 for 5 min` does not say what holds a slot. Looking up by id (not through home's task list) keeps a holder from another project named when home is filtered. The broker stays free of store access. The `compile-watch` fingerprint does not need the fields, since a title does not change while a slot is held.

**Trust.** No `PROTECTED` file changed.
