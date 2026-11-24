<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# 1037: Windows test hook permissions

## Decision

Executable bits in test helpers are set under `#[cfg(unix)]`, as `reject_first` does. The Windows job only builds the tests, so the hook not being executable there does not matter.
