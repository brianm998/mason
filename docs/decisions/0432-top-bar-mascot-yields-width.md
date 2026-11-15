<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0432: the top-bar mascot yields width

Decision: the mascot is decorative, so it is not part of the top bar's minimum width. Its frame has minimum 0 and a low layout priority; it draws at 85% of the illustration when room allows.

Why: a fixed picture pushed the minimum to 1079.6 pt, over the 1000 pt window minimum.
