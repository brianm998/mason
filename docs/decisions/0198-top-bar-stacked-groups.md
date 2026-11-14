# 0198: The top bar stacks its groups and shows both usage windows

The one-row top bar needed a very wide window.

**Decision.** Home sits over back/forward; the attention indicator over the project filter (the filter alone, centred, when nothing needs the owner); usage shows 5-hour above weekly; the four buttons form a 2x2 grid with the wand's cell left empty when there is no self project. Zero counts are gray (`muted`). Both clients, same ids; extra usage lines use `topbar-usage:<account>:<n>`.

**Daemon.** A subscription account pushes a `top_bar` line for each reported main window, shortest first, instead of only the most constrained. Per-model windows (`*_opus`) are left to the Usage page. The wire format is unchanged.

**Trust.** `clients/features.toml` descriptions are out of date; new wording is proposed in `docs/stages/s203.md` and waits for the owner.
