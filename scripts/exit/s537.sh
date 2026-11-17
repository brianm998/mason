#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s537 exit test: the Swift caches exist and are used (static check; the Swift tests run on a Mac).
set -euo pipefail
cd "$(dirname "$0")/../.."
S=clients/swift/Sources
grep -q 'imageLoads' $S/MasonMascot/Mascot.swift
grep -q 'linkRevision' $S/MonitorCore/Model.swift
grep -q 'voiceCache' $S/MonitorCore/Speech.swift
grep -q 'osVoices' $S/MasonMonitor/Speech.swift
echo "s537: caches present"
