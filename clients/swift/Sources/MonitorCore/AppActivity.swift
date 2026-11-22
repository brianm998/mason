// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// Opts the app out of App Nap. A napped monitor runs its main thread at scheduler priority 4
/// (darwinbg) while its window is behind another app, which is below every runnable compile
/// thread on a busy build machine, so a small SwiftUI update waits seconds for a core and alerts
/// are late. The activity is taken once and never ended while the app runs.
public final class AppActivity {
    /// The options held: user-initiated work that still lets the idle system sleep, and low
    /// latency. Never `.idleDisplaySleepDisabled`: the display must still sleep.
    public static let options: ProcessInfo.ActivityOptions = [.userInitiatedAllowingIdleSystemSleep, .latencyCritical]
    public static let reason = "MASON monitor: live pipeline view and spoken alerts"

    private var token: NSObjectProtocol?

    public init() {}

    /// True once an activity is held.
    public var isHeld: Bool { token != nil }

    /// Begins the activity through `begin` the first time; later calls do nothing.
    public func hold(begin: (ProcessInfo.ActivityOptions, String) -> NSObjectProtocol = { ProcessInfo.processInfo.beginActivity(options: $0, reason: $1) }) {
        guard token == nil else { return }
        token = begin(Self.options, Self.reason)
    }
}
