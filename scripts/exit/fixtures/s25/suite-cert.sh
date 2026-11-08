#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# A device suite whose signing certificate has expired while FLAG exists.
# Usage: suite-cert.sh LOG FLAG
log=$1
flag=$2
echo "suite ${MASON_DEVICE:-none}" >> "$log"
if [ -f "$flag" ]; then
  echo "error: The certificate used to sign the app has expired"
  echo "Provisioning profile \"Sample Dev\" has expired."
  exit 1
fi
echo "1..3"; echo "ok 1 - one"; echo "ok 2 - two"; echo "ok 3 - three"
