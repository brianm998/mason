// SPDX-License-Identifier: GPL-3.0-or-later

import Observation

/// Counts the changes `read` would be told about by the Observation framework: the stand-in for a
/// Combine subscription on a property. `read` touches the properties to watch; each change to one
/// of them counts once and the watch is armed again.
@MainActor
final class ChangeCounter {
    private(set) var count = 0
    private var stopped = false
    private let read: () -> Void

    init(_ read: @escaping () -> Void) {
        self.read = read
        arm()
    }

    private func arm() {
        withObservationTracking(read) { [weak self] in
            MainActor.assumeIsolated {
                guard let self, !self.stopped else { return }
                self.count += 1
                self.arm()
            }
        }
    }

    func cancel() { stopped = true }
}
