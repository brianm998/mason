#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Wrap a 256x256 PNG in a Windows .ico (ICO may hold PNG data directly).
# Regenerate packaging/icons/mason-monitor.ico with:
#   sips -z 256 256 packaging/icons/mason-monitor.png --out /tmp/i256.png
#   python3 packaging/make-ico.py /tmp/i256.png packaging/icons/mason-monitor.ico
import struct, sys
png = open(sys.argv[1], "rb").read()
hdr = struct.pack("<HHH", 0, 1, 1)
ent = struct.pack("<BBBBHHII", 0, 0, 0, 0, 1, 32, len(png), 22)
open(sys.argv[2], "wb").write(hdr + ent + png)
