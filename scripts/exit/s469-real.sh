#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# s469-real: the owner's check of the Android phone app on a real phone over Wi-Fi. Not run by the
# pipeline: it needs a phone, the same Wi-Fi as the Mac, and the owner. See docs/stages/s469.md.
set -euo pipefail
echo "s469-real is a checklist for the owner; nothing here can pass on a machine alone:"
cat <<'LIST'
 1. Install: in clients/compose run  gradle -Pmason.android=true :android:installDebug  (phone attached, USB debugging on).
 2. On the Mac: mason lan on, then mason pair (it shows a QR code and an 8-digit code).
 3. In the app: Scan the QR code; it pairs and the sidebar shows the counts. Repeat with Nearby and the typed code
    (the warning says scanning is the stronger check), and with
    adb shell am start -a android.intent.action.VIEW -d 'mason://pair?...'
 4. Open a task with a question; answer with an option and with Other. Cancel, Skip testing and Land now on tasks that allow them.
 5. Home: the session limit - and + change the limit. Mute and Unmute.
 6. Send the app to the background for a minute and bring it back: it reconnects and shows what it missed.
 7. mason devices revoke <name> on the Mac: the app returns to the pairing screen. Pair again; Daemon > Forget does the same.
 8. Confirm the token is not readable in the app's shared_prefs (adb shell run-as <application id> cat shared_prefs/mason.xml).
LIST
echo "RESULT s469-real: checklist printed, not run; record each step on a real phone"
exit 1
