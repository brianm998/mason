<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1226: a node watches every volume its jobs write to

Decision: add `min_other_disk_free_gb`, default 10 GB, for the home, cargo-home and temporary
volumes; the data and build-cache volumes keep `min_disk_free_gb` (50). Reason: the internal disk
holds little that grows, and a healthy node at 48 GB free there must keep working. Readings are per
volume (by volume id), so paths sharing a disk are read once. Unreadable volumes are reported, never
blocking. The limits are tested with a fake disk source; `s1123` no longer depends on the host's disk.
