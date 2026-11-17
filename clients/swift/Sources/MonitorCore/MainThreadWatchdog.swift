// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// The numbers for one window of the watchdog's pings.
public struct LatenessWindow: Equatable, Sendable {
    public var pings = 0
    public var late = 0
    public var worstMs = 0.0
    public var totalLateMs = 0.0
    public init(pings: Int = 0, late: Int = 0, worstMs: Double = 0, totalLateMs: Double = 0) {
        self.pings = pings; self.late = late; self.worstMs = worstMs; self.totalLateMs = totalLateMs
    }

    /// The Health page's line. The Compose monitor prints the same text.
    public func line(windowMinutes: Int?) -> String {
        let span = windowMinutes.map { "in the last \($0) minutes" } ?? "since launch"
        return "UI thread: worst \(Int(worstMs)) ms late \(span), \(late) late ping\(late == 1 ? "" : "s")"
    }
}

/// How late the main run loop was, collected from the watchdog's pings. Pure, so it can be tested:
/// the clock is passed in, and the rolling window drops pings older than `windowMs`.
public struct LatenessStats: Equatable, Sendable {
    public private(set) var samples = 0
    public private(set) var late = 0
    public private(set) var worstMs = 0.0
    public private(set) var totalLateMs = 0.0
    public let thresholdMs: Double
    public let windowMs: Double
    private var recent: [(at: Double, ms: Double)] = []

    public static let defaultWindowMs = 10 * 60_000.0

    public init(thresholdMs: Double, windowMs: Double = LatenessStats.defaultWindowMs) {
        self.thresholdMs = thresholdMs
        self.windowMs = windowMs
    }

    public static func == (a: LatenessStats, b: LatenessStats) -> Bool {
        a.samples == b.samples && a.late == b.late && a.worstMs == b.worstMs
            && a.totalLateMs == b.totalLateMs && a.thresholdMs == b.thresholdMs && a.windowMs == b.windowMs
    }

    /// Record one ping that came back `ms` later than it was due, at clock time `atMs`; true when it was over the threshold.
    @discardableResult
    public mutating func record(lateMs ms: Double, atMs: Double = 0) -> Bool {
        samples += 1
        worstMs = max(worstMs, ms)
        recent.append((atMs, ms))
        prune(now: atMs)
        guard ms > thresholdMs else { return false }
        late += 1
        totalLateMs += ms
        return true
    }

    private mutating func prune(now: Double) {
        let cut = recent.firstIndex { now - $0.at <= windowMs } ?? recent.count
        if cut > 0 { recent.removeFirst(cut) }
    }

    /// Everything since the stats were made.
    public var sinceLaunch: LatenessWindow {
        LatenessWindow(pings: samples, late: late, worstMs: worstMs, totalLateMs: totalLateMs)
    }

    /// The pings in the last `windowMs` before `now`.
    public func window(now: Double) -> LatenessWindow {
        var w = LatenessWindow()
        for r in recent where now - r.at <= windowMs {
            w.pings += 1
            w.worstMs = max(w.worstMs, r.ms)
            if r.ms > thresholdMs { w.late += 1; w.totalLateMs += r.ms }
        }
        return w
    }

    public var summary: String {
        "main thread: \(samples) pings, \(late) late (> \(Int(thresholdMs)) ms), worst \(Int(worstMs)) ms, " +
            "\(Int(totalLateMs)) ms late in all"
    }
}

/// How often the watchdog pings and what it calls late. Release builds ping every 250 ms and call 100 ms
/// late; debug builds, or `MASON_WATCHDOG=1`, ping every 20 ms and call 50 ms late.
public struct WatchdogConfig: Sendable, Equatable {
    public var intervalMs: Double
    public var thresholdMs: Double
    /// A ping later than this is logged to stderr.
    public var logAboveMs: Double

    public init(intervalMs: Double, thresholdMs: Double, logAboveMs: Double) {
        self.intervalMs = intervalMs; self.thresholdMs = thresholdMs; self.logAboveMs = logAboveMs
    }

    public static let release = WatchdogConfig(intervalMs: 250, thresholdMs: 100, logAboveMs: 1000)
    public static let debug = WatchdogConfig(intervalMs: 20, thresholdMs: 50, logAboveMs: 50)

    public static func current(env: [String: String] = ProcessInfo.processInfo.environment) -> WatchdogConfig {
        #if DEBUG
        return .debug
        #else
        return env["MASON_WATCHDOG"] == "1" ? .debug : .release
        #endif
    }
}

/// A background timer on a utility queue asks the main queue to answer every `intervalMs`, and keeps how
/// late the answers were (since launch and over the last 10 minutes). It runs in every build; the
/// numbers are shown on the Health page, and a ping more than a second late is logged to stderr.
public final class MainThreadWatchdog: @unchecked Sendable {
    public static let shared = MainThreadWatchdog()

    private let lock = NSLock()
    private var stats = LatenessStats(thresholdMs: 100)
    private var config = WatchdogConfig.release
    private var timer: DispatchSourceTimer?
    private let clock: @Sendable () -> Double
    private var log: @Sendable (String) -> Void = { FileHandle.standardError.write(Data(("[watchdog] " + $0 + "\n").utf8)) }

    public init(clock: @escaping @Sendable () -> Double = { Double(DispatchTime.now().uptimeNanoseconds) / 1e6 }) {
        self.clock = clock
    }

    public var isRunning: Bool {
        lock.lock()
        defer { lock.unlock() }
        return timer != nil
    }

    /// Start watching.
    public func start(config: WatchdogConfig = .current(), log: (@Sendable (String) -> Void)? = nil) {
        lock.lock()
        defer { lock.unlock() }
        guard timer == nil else { return }
        self.config = config
        stats = LatenessStats(thresholdMs: config.thresholdMs)
        if let log { self.log = log }
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "mason.watchdog", qos: .utility))
        let every = DispatchTimeInterval.milliseconds(Int(config.intervalMs))
        t.schedule(deadline: .now() + every, repeating: every)
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

    /// The last 10 minutes.
    public func recent() -> LatenessWindow {
        lock.lock()
        defer { lock.unlock() }
        return stats.window(now: clock())
    }

    func note(lateMs ms: Double) {
        lock.lock()
        stats.record(lateMs: ms, atMs: clock())
        let line = "main thread late by \(Int(ms)) ms"
        let out = log
        let over = ms > config.logAboveMs
        lock.unlock()
        if over { out(line) }
    }
}
