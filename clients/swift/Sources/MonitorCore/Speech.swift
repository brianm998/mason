// SPDX-License-Identifier: GPL-3.0-or-later

import Foundation

/// Sound and speech (Part 19), without any sound hardware: a queue of announcements behind a
/// `SpeechSink`. The app's sink plays real sounds and speaks with the OS; tests use a fake.

/// One thing to say: a short sound, then one sentence.
public struct Announcement: Equatable, Sendable {
    public var id: String
    /// The condition (`needs_you`, `failed`, `landed`, `daemon_down`, ...).
    public var kind: String
    public var sentence: String
    public var sound: Bool
    public var speech: Bool

    public init(id: String, kind: String, sentence: String, sound: Bool = true, speech: Bool = true) {
        self.id = id
        self.kind = kind
        self.sentence = sentence
        self.sound = sound
        self.speech = speech
    }

    /// Which family of sound goes with this: "wants a decision" and "broke" differ.
    public var family: String {
        switch kind {
        case "needs_you": return "decision"
        case "failed", "landing", "daemon_down", "credentials", "provider_outage", "stalled", "environment", "runtime", "definition",
             "disk", "blocked", "host_lost", "tracker", "provider_limit", "provider_overage", "provider_budget":
            return "broke"
        default: return "info"
        }
    }
}

/// What actually makes noise. Each call finishes by calling `then` exactly once, when the sound or
/// the sentence has ended; the queue never starts the next thing before that.
@MainActor
public protocol SpeechSink: AnyObject {
    func play(sound: String, then: @escaping () -> Void)
    func speak(_ sentence: String, voice: String, then: @escaping () -> Void)
    /// Cut off whatever is playing (the user muted).
    func stop()
    /// The voices the OS offers, by name.
    func voices() -> [String]
}

/// Which announcements are voiced, which sound each family uses, and which voice speaks.
public struct SpeechSettings: Equatable, Sendable {
    /// Conditions that make noise. Everything else stays visual.
    public var kinds: Set<String>
    /// Sound name per family (`decision`, `broke`, `info`).
    public var sounds: [String: String]
    /// An OS voice name; empty for the system default.
    public var voice: String

    /// Kinds added after the first release. A saved list gains them once (see `Announcer.load`).
    public static let addedKinds: Set<String> = [
        "disk", "environment", "blocked", "host_lost", "runtime", "definition", "tracker", "provider_limit", "provider_overage", "provider_budget",
    ]

    public static let defaultKinds: Set<String> = Set([
        "needs_you", "failed", "landing", "daemon_down", "credentials", "provider_outage", "stalled", "landed",
    ]).union(addedKinds)

    public init(kinds: Set<String> = SpeechSettings.defaultKinds, sounds: [String: String]? = nil, voice: String = "") {
        self.kinds = kinds
        self.sounds = sounds ?? ["decision": "Glass", "broke": "Basso", "info": "Tink"]
        self.voice = voice
    }

    public func sound(for a: Announcement) -> String { sounds[a.family] ?? "Tink" }
}

/// How long the connection must be down before the monitor raises "daemon down" itself.
public let daemonDownAfter: TimeInterval = 15

public let daemonDownSentence = "MASON has lost its daemon; check that masond is running."

@MainActor
public final class Announcer {
    /// What makes the noise. The app replaces the silent default with a real one.
    public var sink: SpeechSink
    public var settings: SpeechSettings { didSet { save() } }
    /// Mute is the user's; the announcer only obeys it. It silences sound and speech and nothing else.
    public var isMuted: () -> Bool
    public private(set) var queue: [Announcement] = []
    public private(set) var speaking: Announcement?
    /// Set once the daemon has answered a claim: from then on its announcements are the sound,
    /// and the monitor's own chime for a newly waiting task would only double it.
    public private(set) var heardFromDaemon = false
    /// This monitor is the one that speaks right now.
    public private(set) var isSpeaker = false
    /// The local alert: the connection has been lost for `daemonDownAfter`. Shown as a banner
    /// whether or not anything is muted.
    public private(set) var daemonDown = false
    private var lostSince: Date?
    private var seen: Set<String> = []
    private var prefs: Preferences?

    public init(sink: SpeechSink, isMuted: @escaping () -> Bool = { false }, preferences: Preferences? = nil) {
        self.sink = sink
        self.isMuted = isMuted
        self.prefs = preferences
        settings = SpeechSettings()
        if let p = preferences { settings = Announcer.load(p) }
    }

    // MARK: the queue

    /// Add to the end. Muted, or not a kind that speaks, it makes no noise (and costs nothing:
    /// the visual alert does not come through here). The same id is voiced once.
    public func enqueue(_ a: Announcement) {
        guard !seen.contains(a.id) else { return }
        seen.insert(a.id)
        guard !isMuted(), settings.kinds.contains(a.kind) else { return }
        queue.append(a)
        pump()
    }

    private func pump() {
        guard speaking == nil, !queue.isEmpty else { return }
        let a = queue.removeFirst()
        speaking = a
        let speakIt = { [weak self] in
            guard let self else { return }
            guard a.speech, !a.sentence.isEmpty else { return self.finish(a) }
            self.sink.speak(a.sentence, voice: self.settings.voice) { [weak self] in self?.finish(a) }
        }
        if a.sound {
            sink.play(sound: settings.sound(for: a)) { speakIt() }
        } else {
            speakIt()
        }
    }

    private func finish(_ a: Announcement) {
        guard speaking == a else { return }
        speaking = nil
        pump()
    }

    /// The user muted: stop what is playing and forget what was waiting.
    public func silence() {
        queue.removeAll()
        if speaking != nil { sink.stop() }
        speaking = nil
    }

    // MARK: the daemon's announcements

    /// The body of `POST /announce/claim` for this monitor.
    public func claimBody(client: String) -> String {
        "{\"client\":\"\(client)\",\"muted\":\(isMuted())}"
    }

    /// The daemon's answer to a claim: whether this monitor speaks, and what to say, in order.
    public func handle(claim json: String) {
        guard let data = json.data(using: .utf8),
              let v = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return }
        heardFromDaemon = true
        isSpeaker = (v["speaker"] as? Bool) ?? false
        guard isSpeaker else { return }
        for x in (v["announcements"] as? [[String: Any]]) ?? [] {
            let id = (x["key"] as? String) ?? String(describing: x["id"] ?? "")
            enqueue(Announcement(
                id: id,
                kind: (x["kind"] as? String) ?? "info",
                sentence: (x["sentence"] as? String) ?? "",
                sound: (x["sound"] as? Bool) ?? true,
                speech: (x["speech"] as? Bool) ?? true
            ))
        }
    }

    // MARK: the lost connection

    /// The connection went down (call on each failed attempt; only the first sets the clock).
    public func connectionLost(at now: Date) {
        if lostSince == nil { lostSince = now }
    }

    /// The connection is back: the alert clears, and a later loss is a new one.
    public func connectionRestored() {
        lostSince = nil
        daemonDown = false
        seen = seen.filter { !$0.hasPrefix("daemon_down:") }
    }

    /// Look at the clock: after `daemonDownAfter` of failed reconnects, raise "daemon down" once.
    public func check(now: Date) {
        guard let t = lostSince, !daemonDown, now.timeIntervalSince(t) >= daemonDownAfter else { return }
        daemonDown = true
        enqueue(Announcement(id: "daemon_down:\(t.timeIntervalSince1970)", kind: "daemon_down", sentence: daemonDownSentence))
    }

    // MARK: settings

    static let kindsKey = "speech.kinds"
    /// Set once a saved list has been given `addedKinds`; after that an untick sticks.
    static let kindsMigratedKey = "speech.kinds.added"
    static let voiceKey = "speech.voice"
    static func soundKey(_ f: String) -> String { "speech.sound.\(f)" }

    static func load(_ p: Preferences) -> SpeechSettings {
        var s = SpeechSettings()
        if let k = p.string(kindsKey) {
            s.kinds = Set(k.split(separator: ",").map(String.init))
            if p.string(kindsMigratedKey) == nil {
                s.kinds.formUnion(SpeechSettings.addedKinds)
                p.setString(s.kinds.sorted().joined(separator: ","), for: kindsKey)
            }
        }
        p.setString("1", for: kindsMigratedKey)
        s.voice = p.string(voiceKey) ?? ""
        for f in ["decision", "broke", "info"] {
            if let n = p.string(soundKey(f)) { s.sounds[f] = n }
        }
        return s
    }

    private func save() {
        guard let p = prefs else { return }
        p.setString(settings.kinds.sorted().joined(separator: ","), for: Announcer.kindsKey)
        p.setString(settings.voice, for: Announcer.voiceKey)
        for (f, n) in settings.sounds { p.setString(n, for: Announcer.soundKey(f)) }
    }

    /// The OS voices, for the settings list.
    public func availableVoices() -> [String] { sink.voices() }
}

/// A sink that makes no noise and completes at once (the default until the app supplies one).
@MainActor
public final class SilentSink: SpeechSink {
    public init() {}
    public func play(sound: String, then: @escaping () -> Void) { then() }
    public func speak(_ sentence: String, voice: String, then: @escaping () -> Void) { then() }
    public func stop() {}
    public func voices() -> [String] { [] }
}
