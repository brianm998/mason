# 0028: Tile label and minimum width

- The daemon sends the tile label (StateCount.label, field 5). needs_attention reads "stuck"; other states keep their name with spaces. Clients fall back to the underscore replacement when it is empty.
- tile_min_width = 64 replaces tile_width = 96. A three-digit count at text_count (24, semibold) is about 3 x 14.5 = 44 points wide; plus spacing (8) each side that is about 60, so 64 keeps the width the same from 0 to 999. The ticket's 48 to 56 would be too narrow for three digits. This is an estimate from font metrics, not measured on a rendered screen (see docs/stages/s50.md).
