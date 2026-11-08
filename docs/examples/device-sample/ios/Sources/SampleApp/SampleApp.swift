// SPDX-License-Identifier: GPL-3.0-or-later

import SwiftUI

/// What the screen says. Kept out of the view so a test can check it without a screen.
public func greeting(for name: String) -> String {
    name.isEmpty ? "Hello, stranger" : "Hello, \(name)"
}

public struct GreetingView: View {
    @State private var name = ""
    public init() {}
    public var body: some View {
        VStack(spacing: 12) {
            TextField("Name", text: $name)
                .textFieldStyle(.roundedBorder)
                .accessibilityIdentifier("name")
            Text(greeting(for: name))
                .accessibilityIdentifier("greeting")
        }
        .padding()
    }
}
