<!-- SPDX-License-Identifier: GPL-3.0-or-later --># 0470: Shorten the hold tests by editing the stored hold, not the production default

Status: accepted

The kill test needs a long hold so a slow restart still finds it in force, then needs it to end.
It now rewrites the hold's end time in the store once the restart has been checked. The scheduler's
constants stay as they are. Not verified by a run here: the interactive build was refused by the
build queue, so the change is checked by the pipeline's test step only.
