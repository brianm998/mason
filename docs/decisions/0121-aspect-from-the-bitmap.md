# 0121: The AppKit mascot loader takes its aspect from the bitmap

**Decision.** `points` is the image's width in points; height follows the pixel aspect. Chosen over "long side" because every call site thinks in widths (768 for the 3:2 brand image, 512 for square art, which is identical either way).

**Mismatched reps.** A rep of another aspect than the first is skipped, never stretched.

**No call-site fix.** The loader owns the aspect; views stay as they are.

**Trust.** No protected path or `features.toml` change.

**Not verified:** no screenshot taken; see the stage note.
