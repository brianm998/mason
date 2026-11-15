# 0350: a link looks like a link, from one definition per app

**Decision.** Anything that opens a task's page or the tracker is drawn in `theme.link` (its own token, not the accent), underlined, with the pointing hand on hover. Each app has one definition (Swift `TaskLink`/`WebLink`/`linkStyle()`, Compose `Link()`), and a grep-style test forbids the old styles. A whole clickable row keeps the hand and puts the link look on the words that name the task, never the whole paragraph. Things that are not navigation keep the button look.

**Why.** Nothing on screen said a task number was clickable. The accent could not serve: it also tints selected rows. `#0000EE` on light and `#8AB4F8` on dark give at least 4.5:1 on background and surface (tested).

**Trust.** `scripts/exit/s357.sh` is a new exit script. `clients/features.toml` is untouched.
