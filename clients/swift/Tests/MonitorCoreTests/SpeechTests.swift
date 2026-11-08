// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import XCTest

@testable import MonitorCore

/// Records what it is asked to do and finishes only when the test says so, so a test can see
/// that the next thing waits for the last.
@MainActor
final class FakeSink: SpeechSink {
    var log: [String] = []
    var pending: [() -> Void] = []
    var stopped = 0
    func play(sound: String, then: @escaping () -> Void) {
        log.append("sound:\(sound)")
        pending.append(then)
    }
    func speak(_ sentence: String, voice: String, then: @escaping () -> Void) {
        log.append("say:\(sentence)|\(voice)")
        pending.append(then)
    }
    func stop() { stopped += 1; pending.removeAll() }
    func voices() -> [String] { ["Samantha", "Daniel"] }
    /// The current sound or sentence ends.
    func finish() { let f = pending.removeFirst(); f() }
}

@MainActor
final class SpeechTests: XCTestCase {
    func announcer(_ sink: FakeSink, muted: @escaping () -> Bool = { false }) -> Announcer {
        Announcer(sink: sink, isMuted: muted, preferences: MemoryPreferences())
    }

    func ann(_ id: String, _ kind: String, _ s: String) -> Announcement {
        Announcement(id: id, kind: kind, sentence: s)
    }

    func testTwoAnnouncementsQueueAndNeverOverlap() {
        let sink = FakeSink()
        let a = announcer(sink)
        a.enqueue(ann("1", "needs_you", "Web API 204 needs a decision on token format."))
        a.enqueue(ann("2", "failed", "Task 7 failed."))
        // Only the first sound has started; the second announcement waits.
        XCTAssertEqual(sink.log, ["sound:Glass"])
        XCTAssertEqual(a.queue.count, 1)
        sink.finish()
        XCTAssertEqual(sink.log, ["sound:Glass", "say:Web API 204 needs a decision on token format.|"])
        sink.finish()  // the sentence ends; now the second begins, with its own sound
        XCTAssertEqual(sink.log.last, "sound:Basso")
        sink.finish()
        XCTAssertEqual(sink.log.last, "say:Task 7 failed.|")
        sink.finish()
        XCTAssertNil(a.speaking)
        XCTAssertTrue(a.queue.isEmpty)
    }

    func testDecisionAndBrokeSoundDifferent() {
        let s = SpeechSettings()
        XCTAssertNotEqual(s.sound(for: ann("a", "needs_you", "x")), s.sound(for: ann("b", "failed", "x")))
    }

    func testTheSameAnnouncementIsVoicedOnce() {
        let sink = FakeSink()
        let a = announcer(sink)
        a.enqueue(ann("1", "failed", "Task 7 failed."))
        a.enqueue(ann("1", "failed", "Task 7 failed."))
        XCTAssertEqual(a.queue.count, 0)
        sink.finish(); sink.finish()
        XCTAssertEqual(sink.log.count, 2)
    }

    func testMutingSilencesSoundAndSpeech() {
        let sink = FakeSink()
        var muted = false
        let a = announcer(sink, muted: { muted })
        a.enqueue(ann("1", "needs_you", "One."))
        a.enqueue(ann("2", "needs_you", "Two."))
        muted = true
        a.silence()
        XCTAssertEqual(sink.stopped, 1)
        XCTAssertTrue(a.queue.isEmpty)
        a.enqueue(ann("3", "failed", "Three."))
        XCTAssertTrue(a.queue.isEmpty)
        XCTAssertEqual(sink.log, ["sound:Glass"], "nothing new started while muted")
    }

    func testMutingLeavesTheVisualAlerts() {
        let m = MonitorModel()
        var h = HeaderView()
        h.attention = 2
        h.status = "2 need you"
        m.header = h
        m.connected = true
        var ban = false
        m.muted = true
        ban = m.attention == 2 && m.header?.status == "2 need you"
        XCTAssertTrue(ban, "the attention indicator and status are drawn from the views, not from the sound")
        // And the lost connection is still raised while muted.
        let t0 = Date()
        m.disconnected(now: t0)
        m.pulse(now: t0.addingTimeInterval(16))
        XCTAssertTrue(m.announcer.daemonDown)
    }

    func testUnlistedKindsStayVisualOnly() {
        let sink = FakeSink()
        let a = announcer(sink)
        a.settings.kinds.remove("landed")
        a.enqueue(ann("1", "landed", "Task 7 landed."))
        XCTAssertTrue(sink.log.isEmpty)
    }

    func testVoiceAndSoundsAreConfigurableAndRemembered() {
        let prefs = MemoryPreferences()
        let sink = FakeSink()
        let a = Announcer(sink: sink, preferences: prefs)
        XCTAssertEqual(a.availableVoices(), ["Samantha", "Daniel"])
        a.settings.voice = "Daniel"
        a.settings.sounds["decision"] = "Ping"
        a.enqueue(ann("1", "needs_you", "Hello."))
        sink.finish()
        XCTAssertEqual(sink.log, ["sound:Ping", "say:Hello.|Daniel"])
        let again = Announcer(sink: sink, preferences: prefs)
        XCTAssertEqual(again.settings.voice, "Daniel")
        XCTAssertEqual(again.settings.sounds["decision"], "Ping")
    }

    func testDaemonDownIsRaisedAfterFifteenSecondsOnceAndRearms() {
        let sink = FakeSink()
        let a = announcer(sink)
        let t0 = Date()
        a.connectionLost(at: t0)
        a.check(now: t0.addingTimeInterval(14))
        XCTAssertFalse(a.daemonDown)
        a.check(now: t0.addingTimeInterval(15))
        XCTAssertTrue(a.daemonDown)
        XCTAssertEqual(sink.log, ["sound:Basso"])
        sink.finish()
        XCTAssertEqual(sink.log.last, "say:\(daemonDownSentence)|")
        sink.finish()
        a.check(now: t0.addingTimeInterval(60))
        XCTAssertEqual(sink.log.count, 2, "once")
        a.connectionRestored()
        XCTAssertFalse(a.daemonDown)
        a.connectionLost(at: t0.addingTimeInterval(100))
        a.check(now: t0.addingTimeInterval(120))
        XCTAssertTrue(a.daemonDown)
        XCTAssertEqual(sink.log.count, 3)
    }

    func testOnlyTheSpeakerQueuesWhatTheDaemonSends() {
        let sink = FakeSink()
        let a = announcer(sink)
        let body = """
        {"speaker":false,"announcements":[{"key":"x","kind":"failed","sentence":"No."}]}
        """
        a.handle(claim: body)
        XCTAssertFalse(a.isSpeaker)
        XCTAssertTrue(sink.log.isEmpty)
        let mine = """
        {"speaker":true,"announcements":[
          {"key":"a","kind":"needs_you","sentence":"First.","sound":true,"speech":true},
          {"key":"b","kind":"failed","sentence":"Second.","sound":true,"speech":true}]}
        """
        a.handle(claim: mine)
        XCTAssertTrue(a.isSpeaker)
        XCTAssertEqual(a.queue.count, 1)
        sink.finish(); sink.finish()
        XCTAssertEqual(sink.log.last, "sound:Basso")
    }

    func testTheClaimCarriesTheMuteState() {
        let sink = FakeSink()
        let a = announcer(sink, muted: { true })
        XCTAssertTrue(a.claimBody(client: "c1").contains("\"muted\":true"))
        XCTAssertTrue(a.claimBody(client: "c1").contains("\"client\":\"c1\""))
    }

    func testTheOldChimeStopsOnceTheDaemonSpeaksForItself() {
        let m = MonitorModel()
        var chimes = 0
        m.onAttention = { chimes += 1 }
        func sidebar(_ ids: [UInt64]) -> SidebarView {
            var r = SidebarView()
            var s = SidebarSection()
            s.state = "needs_attention"
            s.rows = ids.map { i in var t = TaskRow(); t.id = i; return t }
            r.sections = [s]
            return r
        }
        m.noteAttention(sidebar([]))
        m.noteAttention(sidebar([1]))
        XCTAssertEqual(chimes, 1)
        m.announcer.handle(claim: "{\"speaker\":false,\"announcements\":[]}")
        m.noteAttention(sidebar([1, 2]))
        XCTAssertEqual(chimes, 1)
    }
}
