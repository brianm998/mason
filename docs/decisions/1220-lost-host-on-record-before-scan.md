<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1220: the lost-host test waits for the host's record before scanning

The test saw the grandchild run before the host had written its record, so the registry scan
found no host. It now waits, through the harness's bounded `wait_for`, until the host is on
record, then scans and asserts exactly one host. We did not sleep or relax the assertion: a
sleep is a timing guess, and the assertion is what the test is for.
