#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Fails if the given source dirs reference IO, clocks, threads, randomness or
# async runtimes. Usage: check-pure.sh DIR...
set -euo pipefail
pat='std::(time|fs|net|process|thread|env|io|sync::mpsc)|SystemTime|Instant::now|tokio|async_std|rand::|getrandom|println!|eprintln!|dbg!|File::|TcpStream|UdpSocket'
bad=0
for d in "$@"; do
  if grep -rnE "$pat" --include='*.rs' "$d"; then bad=1; fi
done
exit $bad
