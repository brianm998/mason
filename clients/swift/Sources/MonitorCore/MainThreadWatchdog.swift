// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// How late the main run loop was, collected from the watchdog's pings. Pure, so it can be tested.
public struct LatenessStats: Equatable, Sendable {
    public private(set) var samples = 0
    public private(set) var late = 0
    public private(set) var worstMs = 0.0
    public private(set) var totalLateMs = 0.0
    public let thresholdMs: Double

    public init(thresholdMs: Double) { self.thresholdMs = thresholdMs }

    /// Record one ping that came back `ms` later than it was due; true when it was over the threshold.
    @discardableResult
    public mutating func record(lateMs ms: Double) -> Bool {
        samples += 1
        worstMs = max(worstMs, ms)
        guard ms > thresholdMs else { return false }
        late += 1
        totalLateMs += ms
        return true
    }

    public var summary: String {
        "main thread: \(samples) pings, \(late) late (> \(Int(thresholdMs)) ms), worst \(Int(worstMs)) ms, " +
            "\(Int(totalLateMs)) ms late in all"
    }
}

/// A debug aid: a background timer asks the main queue to answer every `interval`, and logs when the
/// answer is more than `threshold` late, which is a frozen or stuttering window. Nothing here runs in a
/// release build unless `MASON_WATCHDOG=1` is set. The numbers for a fixture stream are in docs/stages/s332.md.
public final class MainThreadWatchdog: @unchecked Sendable {
    public static let shared = MainThreadWatchdog()

    private let lock = NSLock()
    private var stats = LatenessStats(thresholdMs: 50)
    private var timer: DispatchSourceTimer?
    private var log: @Sendable (String) -> Void = { FileHandle.standardError.write(Data(("[watchdog] " + $0 + "\n").utf8)) }

    public init() {}

    /// Start watching. Does nothing in a release build, unless `MASON_WATCHDOG` is set.
    public func start(
        intervalMs: Double = 20, thresholdMs: Double = 50,
        log: (@Sendable (String) -> Void)? = nil
    ) {
        #if !DEBUG
        guard ProcessInfo.processInfo.environment["MASON_WATCHDOG"] == "1" else { return }
        #endif
        lock.lock()
        defer { lock.unlock() }
        guard timer == nil else { return }
        stats = LatenessStats(thresholdMs: thresholdMs)
        if let log { self.log = log }
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "mason.watchdog", qos: .utility))
        t.schedule(deadline: .now() + .milliseconds(Int(intervalMs)), repeating: .milliseconds(Int(intervalMs)))
        t.setEventHandler { [weak self] in
            let sent = DispatchTime.now().uptimeNanoseconds
            DispatchQueue.main.async {
                let ms = Double(DispatchTime.now().uptimeNanoseconds - sent) / 1e6
                self?.note(lateMs: ms)
            }
        }
        t.resume()
        timer = t
    }

    public func stop() {
        lock.lock()
        timer?.cancel()
        timer = nil
        lock.unlock()
    }

    /// The numbers so far.
    public var report: String {
        lock.lock()
        defer { lock.unlock() }
        return stats.summary
    }

    func note(lateMs ms: Double) {
        lock.lock()
        let over = stats.record(lateMs: ms)
        let line = "main thread late by \(Int(ms)) ms"
        let out = log
        lock.unlock()
        if over { out(line) }
    }
}
