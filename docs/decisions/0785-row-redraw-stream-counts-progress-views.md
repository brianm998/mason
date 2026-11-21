# 0785: the stream test counts progress views, not RowView bodies

Progress messages redraw only progress views by design, so a stream of them costs zero RowView bodies. The test keeps the upper bound on RowView evaluations and the `before > 0` rows-drawn precondition, and proves the messages were drawn by a progress-view count. Decision 0708's rule against loosening the gate stands: no precondition was removed without a replacement.
