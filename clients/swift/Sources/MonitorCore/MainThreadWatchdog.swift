// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation
#if canImport(AppKit)
import AppKit
#endif

/// Whether the owner could see the app when a ping was answered.
public enum AppVisibility: Equatable, Sendable {
    /// The app is frontmost.
    case frontmost
    /// On screen but not frontmost: drawing is still seen.
    case visible
    /// Hidden, minimised or occluded: macOS may nap it, so lateness here is not slow drawing.
    case hidden

    public var isBackground: Bool { self == .hidden }
}

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

    /// The smaller line for stalls taken while the app was in the background. The Compose monitor prints the same text.
    public func backgroundLine(windowMinutes: Int?) -> String {
        let span = windowMinutes.map { "in the last \($0) minutes" } ?? "since launch"
        return "In the background: worst \(Int(worstMs)) ms late \(span), \(late) late ping\(late == 1 ? "" : "s")"
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
    private var recent: [(at: Double, ms: Double, background: Bool)] = []
    private var bgSamples = 0
    private var bgLate = 0
    private var bgWorstMs = 0.0
    private var bgTotalLateMs = 0.0

    public static let defaultWindowMs = 10 * 60_000.0

    public init(thresholdMs: Double, windowMs: Double = LatenessStats.defaultWindowMs) {
        self.thresholdMs = thresholdMs
        self.windowMs = windowMs
    }

    public static func == (a: LatenessStats, b: LatenessStats) -> Bool {
        a.samples == b.samples && a.bgSamples == b.bgSamples && a.late == b.late && a.worstMs == b.worstMs
            && a.totalLateMs == b.totalLateMs && a.thresholdMs == b.thresholdMs && a.windowMs == b.windowMs
    }

    /// Record one ping that came back `ms` later than it was due, at clock time `atMs`; true when it was over the threshold.
    /// Samples taken while the app was hidden count in the background numbers, not the visible ones.
    @discardableResult
    public mutating func record(lateMs ms: Double, atMs: Double = 0, app: AppVisibility = .frontmost) -> Bool {
        let bg = app.isBackground
        recent.append((atMs, ms, bg))
        prune(now: atMs)
        if bg {
            bgSamples += 1
            bgWorstMs = max(bgWorstMs, ms)
            guard ms > thresholdMs else { return false }
            bgLate += 1
            bgTotalLateMs += ms
            return true
        }
        samples += 1
        worstMs = max(worstMs, ms)
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

    /// Everything taken in the background since the stats were made.
    public var backgroundSinceLaunch: LatenessWindow {
        LatenessWindow(pings: bgSamples, late: bgLate, worstMs: bgWorstMs, totalLateMs: bgTotalLateMs)
    }

    /// The visible pings in the last `windowMs` before `now`.
    public func window(now: Double) -> LatenessWindow { window(now: now, background: false) }

    /// The background pings in the last `windowMs` before `now`.
    public func backgroundWindow(now: Double) -> LatenessWindow { window(now: now, background: true) }

    private func window(now: Double, background: Bool) -> LatenessWindow {
        var w = LatenessWindow()
        for r in recent where now - r.at <= windowMs && r.background == background {
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
/// late the answers were (since launch and over the last 10 minutes). Only one ping is in flight at a time,
/// so one blocked main queue is one sample, not one per tick. It runs in every build; the numbers are
/// shown on the Health page, and a stall of more than a second is logged to stderr.
public final class MainThreadWatchdog: @unchecked Sendable {
    public static let shared = MainThreadWatchdog()

    private let lock = NSLock()
    private var stats = LatenessStats(thresholdMs: 100)
    private var config = WatchdogConfig.release
    private var timer: DispatchSourceTimer?
    private var inFlight = false
    private let clock: @Sendable () -> Double
    private let post: @Sendable (@escaping @Sendable () -> Void) -> Void
    private let appState: @Sendable () -> AppVisibility
    private var log: @Sendable (String) -> Void = { FileHandle.standardError.write(Data(("[watchdog] " + $0 + "\n").utf8)) }

    /// `post` runs a block on the main queue and `appState` is read on the main queue when a ping is answered;
    /// tests pass a fake queue and state.
    public init(
        clock: @escaping @Sendable () -> Double = { Double(DispatchTime.now().uptimeNanoseconds) / 1e6 },
        post: @escaping @Sendable (@escaping @Sendable () -> Void) -> Void = { DispatchQueue.main.async(execute: $0) },
        appState: @escaping @Sendable () -> AppVisibility = MainThreadWatchdog.currentAppVisibility
    ) {
        self.clock = clock
        self.post = post
        self.appState = appState
    }

    /// The real app's state. Call on the main thread.
    public static func currentAppVisibility() -> AppVisibility {
        #if canImport(AppKit)
        return MainActor.assumeIsolated {
            if NSApp == nil { return .frontmost }
            if !NSApp.occlusionState.contains(.visible) { return .hidden }
            return NSApp.isActive ? .frontmost : .visible
        }
        #else
        return .frontmost
        #endif
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
        armLocked(config: config, log: log)
        let t = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "mason.watchdog", qos: .utility))
        let every = DispatchTimeInterval.milliseconds(Int(config.intervalMs))
        t.schedule(deadline: .now() + every, repeating: every)
        t.setEventHandler { [weak self] in self?.tick() }
        t.resume()
        timer = t
    }

    /// Configure without a timer, so tests can drive `tick()` by hand with no
    /// background queue touching their state.
    func arm(config: WatchdogConfig, log: (@Sendable (String) -> Void)? = nil) {
        lock.lock()
        defer { lock.unlock() }
        armLocked(config: config, log: log)
    }

    private func armLocked(config: WatchdogConfig, log: (@Sendable (String) -> Void)?) {
        self.config = config
        stats = LatenessStats(thresholdMs: config.thresholdMs)
        inFlight = false
        if let log { self.log = log }
    }

    public func stop() {
        lock.lock()
        timer?.cancel()
        timer = nil
        lock.unlock()
    }

    /// One timer tick: send a ping unless the last one has not come back yet.
    func tick() {
        lock.lock()
        if inFlight { lock.unlock(); return }
        inFlight = true
        let sent = clock()
        lock.unlock()
        post { [weak self] in
            guard let self else { return }
            let ms = self.clock() - sent
            let state = self.appState()
            self.lock.lock()
            self.inFlight = false
            self.lock.unlock()
            self.note(lateMs: ms, app: state)
        }
    }

    /// The numbers so far.
    public var report: String {
        lock.lock()
        defer { lock.unlock() }
        return stats.summary
    }

    /// The last 10 minutes, while the app was visible.
    public func recent() -> LatenessWindow {
        lock.lock()
        defer { lock.unlock() }
        return stats.window(now: clock())
    }

    /// The last 10 minutes, while the app was in the background.
    public func recentBackground() -> LatenessWindow {
        lock.lock()
        defer { lock.unlock() }
        return stats.backgroundWindow(now: clock())
    }

    func note(lateMs ms: Double, app: AppVisibility = .frontmost) {
        lock.lock()
        stats.record(lateMs: ms, atMs: clock(), app: app)
        let line = "main thread blocked for \(Int(ms)) ms" + (app.isBackground ? " (app in background)" : "")
        let out = log
        let over = ms > config.logAboveMs
        lock.unlock()
        if over { out(line) }
    }
}
