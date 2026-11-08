// SPDX-License-Identifier: GPL-3.0-or-later

import AVFoundation
import AppKit
import MonitorCore

/// Plays the named system sound, then speaks with an OS voice. Each call finishes when the sound
/// or the sentence has ended, which is what keeps announcements from overlapping.
@MainActor
final class SystemSink: NSObject, SpeechSink, NSSoundDelegate, AVSpeechSynthesizerDelegate {
    private let synth = AVSpeechSynthesizer()
    private var sound: NSSound?
    private var soundDone: (() -> Void)?
    private var speechDone: (() -> Void)?

    override init() {
        super.init()
        synth.delegate = self
    }

    func play(sound name: String, then: @escaping () -> Void) {
        guard let s = NSSound(named: NSSound.Name(name)) else { NSSound.beep(); return then() }
        sound = s
        soundDone = then
        s.delegate = self
        if !s.play() { soundDone = nil; then() }
    }

    nonisolated func sound(_ sound: NSSound, didFinishPlaying: Bool) {
        Task { @MainActor in
            let f = self.soundDone
            self.soundDone = nil
            f?()
        }
    }

    func speak(_ sentence: String, voice: String, then: @escaping () -> Void) {
        let u = AVSpeechUtterance(string: sentence)
        if !voice.isEmpty {
            u.voice = AVSpeechSynthesisVoice.speechVoices().first { $0.name == voice }
        }
        speechDone = then
        synth.speak(u)
    }

    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didFinish u: AVSpeechUtterance) {
        Task { @MainActor in
            let f = self.speechDone
            self.speechDone = nil
            f?()
        }
    }

    nonisolated func speechSynthesizer(_ s: AVSpeechSynthesizer, didCancel u: AVSpeechUtterance) {
        Task { @MainActor in self.speechDone = nil }
    }

    func stop() {
        sound?.stop()
        soundDone = nil
        synth.stopSpeaking(at: .immediate)
        speechDone = nil
    }

    func voices() -> [String] {
        Array(Set(AVSpeechSynthesisVoice.speechVoices().map(\.name))).sorted()
    }
}
