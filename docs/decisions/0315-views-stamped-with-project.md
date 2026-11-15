# 0315: project-scoped views carry the project they were computed for

**Decision.** `HeaderView`, `SidebarView`, `HomeView` and `LandingView` each carry `project` (empty: all projects). The client model drops a view whose project is not its selection, and clears project-scoped values when the selection changes.

**Why.** The client cannot otherwise tell a late rebuild for the old filter from the new one, and a held-over `landing` view showed another project's queue on home. Dropping at the model keeps the rule in one place for every page that reads it.

**Trust.** No file in `PROTECTED` is changed. `scripts/exit/s321.sh` is new. Machine-wide figures (CPU, compilation, sessions) stay unfiltered and are labeled "all projects".
