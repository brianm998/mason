#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Build packaging/icons/mason-monitor.icns from the macOS-shaped brand art. The .icns holds the
# 1024x1024 PNG directly (type ic10), so the file is a pure function of the source art.
#   python3 packaging/make-icns.py            write the .icns
#   python3 packaging/make-icns.py --check    fail if the committed .icns has drifted
import os, struct, sys

here = os.path.dirname(os.path.abspath(__file__))
src = os.path.join(here, "../../swift/Sources/MasonMascot/Art/brand/app_icon_macos.png")
dst = os.path.join(here, "icons/mason-monitor.icns")

png = open(src, "rb").read()
body = b"ic10" + struct.pack(">I", len(png) + 8) + png
icns = b"icns" + struct.pack(">I", len(body) + 8) + body

if "--check" in sys.argv:
    if not os.path.exists(dst) or open(dst, "rb").read() != icns:
        sys.exit("mason-monitor.icns is out of date: run packaging/make-icns.py")
else:
    open(dst, "wb").write(icns)
