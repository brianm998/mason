#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
#
# SSH_ASKPASS helper for masond. When the owner picks an SSH key that has a passphrase, masond
# runs `ssh-add --apple-use-keychain <key>` with SSH_ASKPASS pointing here and
# SSH_ASKPASS_REQUIRE=force. ssh-add passes the prompt as $1 ("Enter passphrase for /path/key:").
# The passphrase goes dialog -> ssh-add -> Keychain; it never passes through MASON, a protocol
# message, a log or the store.
#
# Cancel exits non-zero, which ends ssh-add with a failure.
prompt="${1:-Passphrase}"
# AppleScript string: escape backslash and double quote.
escaped=$(printf '%s' "$prompt" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')
exec /usr/bin/osascript \
  -e "text returned of (display dialog \"$escaped\" default answer \"\" with hidden answer with title \"MASON\" buttons {\"Cancel\", \"OK\"} default button \"OK\")"
