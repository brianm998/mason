<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0475: The hold kill test waits for both holds before it kills

Status: accepted

The test killed the daemon as soon as the status view read "blocked". The view reads the usage
snapshot, which is written before the hold is stored, and only the first task had met the limit. A
second task's first develop session could then run after the hold was lifted, hit the limit again
and re-arm the hold. The test now waits with `wait_for` until the store holds an account hold and
each task has two launch rows. The store edit and the fixture counts are unchanged, since the
fixture already gives each task's first develop session the limit turn.
