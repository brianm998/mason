# 0142: The sidebar fits its panel; pieces give way, nothing clips

The scrolling content took its ideal width, so a row wider than the panel was centred and clipped on both edges. The content is now framed to the panel width, leading-aligned, so overflow is settled by children shrinking.

**Metadata line: `ViewThatFits`, not a flow `Layout`.** There are only four useful forms (all; without the model; without the typical range; duration and icon over the cells), and `ViewThatFits` is what `ProgressLine` already uses. A custom `Layout` would be more code for the same result. Order of giving way: model, typical range, then wrap.

**`ProgressLine`** gains a last form where count and tokens share one truncating text and only the time is fixed. The accessibility label is unchanged.

**Header and key:** the section title truncates, the count keeps its width; the row key never shrinks.

**Compose** got the same priorities (typical and title ellipsize, key stays whole). It was not built or run here.
