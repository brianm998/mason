<!-- SPDX-License-Identifier: GPL-3.0-or-later -->

# 1108: node host folders are named by a key; local ones keep their ids

A step host's socket path must be at most 100 characters (`sun_path` is 104), and the limit is not raised. On a node the folder name was `<coord>-<id>-<phase>`, so the documented data directory could run no job. The folder is now a 16-character hash of the three, with the readable name in a file beside it.

The coordinator's own host folders (`<data-dir>/h/<id>`) are not hashed. Task, run and batch numbers are read back out of those names (`sweep::parse_host_id`, the archive, cleanup, health), so hashing them would change all of those readers. They get the same bound instead: the launcher refuses an over-long path by name and length, and `mason doctor` warns when a long id would not fit. The warning is not a failure, because how long an id gets depends on the project's suite names, and short ids still work.
