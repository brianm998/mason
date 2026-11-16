// SPDX-License-Identifier: GPL-3.0-or-later

import SwiftUI

// The Mac-only `.link` button style and `.checkbox` toggle style, given iOS stand-ins with the
// same names so the shared views compile unchanged: a borderless button, and a switch.
#if !os(macOS)
extension PrimitiveButtonStyle where Self == BorderlessButtonStyle {
    static var link: BorderlessButtonStyle { BorderlessButtonStyle() }
}

extension ToggleStyle where Self == SwitchToggleStyle {
    static var checkbox: SwitchToggleStyle { SwitchToggleStyle() }
}
#endif
