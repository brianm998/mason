<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1202: command-guard tests derive paths from the adapter

Tests of the hook commands written into a session's settings derive the expected binary path from
the adapter's own `mcp_bin` (`with_file_name`) rather than a literal such as `/bin/mason`, so they
hold on Windows, where the separator differs.
