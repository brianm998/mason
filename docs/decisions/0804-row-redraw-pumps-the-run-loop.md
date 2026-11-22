# 0804: Row-redraw tests pump the run loop

Status: accepted

Task #830. 0764 moved the row-redraw tests to ImageRenderer with no run-loop turns, and progress views
stopped being evaluated, failing two tests. Following 0708 (run-loop turns let the display cycle run),
`draw()` drains the main run loop around each image read. No assertion, bound or name changed; no skip added.
