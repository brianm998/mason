<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 0709: the exit-script helper resolves its selector from its own directory

Status: accepted (s716).

Context: the helper ran the selector by a repo-root-relative path and hid its failure, so a helper run in another repository passed with `PASS none`.

Decision: resolve `select-exit-scripts.sh` from the helper's directory and print `FAIL select-exit-scripts` and exit 1 if it fails.

Consequence: s249 passes on Linux. s193 and s219 remain open until the full shard log is read.
