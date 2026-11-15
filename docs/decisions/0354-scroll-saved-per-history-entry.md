# 0354: scroll is saved per history entry, not per page

Decision: each `NavHistory` entry carries its own scroll position, saved when the entry is left and restored only by back or forward. The live position lives in the model, unpublished, and enters the history only on navigation.

Why: the same page can be in the stack twice and each must return to its own place. Publishing every scroll frame through the history would re-render the whole monitor.

Consequence: restoring is a one-shot on arrival, once the page's content is ready, and gives way to the owner's own scrolling.
