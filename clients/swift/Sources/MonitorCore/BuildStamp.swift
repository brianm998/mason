// SPDX-License-Identifier: GPL-3.0-or-later

/// The commit this build was made from; empty for an unstamped build.
/// The daemon's monitor build and `make` write this file before they compile and `make`
/// puts it back afterwards, so a checked-in tree never carries a commit.
public enum BuildStamp {
    public static let commit = ""
}
