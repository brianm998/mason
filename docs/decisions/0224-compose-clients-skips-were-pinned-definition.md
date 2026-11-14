# 0224: compose-clients force skips came from pinned old definitions; the suite is not changed

Seven force skips of `compose-clients` (tasks 186, 219, 225, 230, 245, 256, 267) all carry one reason: a pinned definition's parser counted 0 tests. 812a7849 already fixed the command and parser. The suite is right for the work and passes here, so neither the suite nor any gate is changed or weakened. See docs/stages/s229.md.
