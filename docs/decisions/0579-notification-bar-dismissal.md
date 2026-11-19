# 0579: notification dismissals are client-side, keyed by what a row is

Dismissals are stored per device in `Preferences`, keyed by content (client id, or kind+project+mode+session+text, or issue number), so a changed text is a new notification and shows again. Keys of rows that are gone are forgotten. No daemon or protocol change. A collapsed-only pause row is dismissed as one row keyed by its pauses. The height is clamped to one row .. all rows .. 60% of the window.
