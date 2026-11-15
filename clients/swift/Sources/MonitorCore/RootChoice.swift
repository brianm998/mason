// SPDX-License-Identifier: GPL-3.0-or-later

/// Which root layout the iOS app shows. A regular horizontal size class (iPad) gets the
/// monitor layout, a compact one (iPhone) gets the phone screens; an unknown class is
/// treated as compact, the smaller and safer layout.
public enum RootChoice: Equatable, Sendable {
    case pad
    case phone

    public static func choose(regularWidth: Bool?) -> RootChoice {
        regularWidth == true ? .pad : .phone
    }
}
