#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Build the Android launcher icon resources from the brand art: the adaptive icon's foreground is
# app_icon.png (inset into the safe zone), its background the art's own corner colour.
#   python3 packaging/make-android-icons.py            write the files
#   python3 packaging/make-android-icons.py --check    fail if any file has drifted
import os, struct, sys, zlib

here = os.path.dirname(os.path.abspath(__file__))
src = os.path.join(here, "../../swift/Sources/MasonMascot/Art/brand/app_icon.png")
res = os.path.join(here, "../android/src/main/res")

art = open(src, "rb").read()

# Pixel (0,0) is the first three bytes after the first scanline's filter byte, for every PNG filter.
def corner(png):
    i, idat = 8, b""
    while i < len(png):
        n, kind = struct.unpack(">I4s", png[i:i + 8])
        if kind == b"IDAT":
            idat += png[i + 8:i + 8 + n]
        i += 12 + n
    raw = zlib.decompressobj().decompress(idat, 16)
    return "#%02X%02X%02X" % tuple(raw[1:4])

SPDX = "<!-- SPDX-License-Identifier: GPL-3.0-or-later -->\n"
adaptive = SPDX + """<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background" />
    <foreground android:drawable="@drawable/ic_launcher_foreground" />
</adaptive-icon>
"""
files = {
    "drawable-nodpi/ic_launcher_art.png": art,
    "drawable/ic_launcher_foreground.xml": (SPDX + """<inset xmlns:android="http://schemas.android.com/apk/res/android"
    android:drawable="@drawable/ic_launcher_art"
    android:inset="18%" />
""").encode(),
    "values/ic_launcher_background.xml": (SPDX + """<resources>
    <color name="ic_launcher_background">%s</color>
</resources>
""" % corner(art)).encode(),
    "mipmap-anydpi/ic_launcher.xml": adaptive.encode(),
    "mipmap-anydpi/ic_launcher_round.xml": adaptive.encode(),
}

bad = []
for rel, data in files.items():
    path = os.path.join(res, rel)
    if "--check" in sys.argv:
        if not os.path.exists(path) or open(path, "rb").read() != data:
            bad.append(rel)
    else:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        open(path, "wb").write(data)
if bad:
    sys.exit("out of date, run packaging/make-android-icons.py: " + ", ".join(bad))
