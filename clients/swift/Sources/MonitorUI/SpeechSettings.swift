// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// Which conditions make noise, which sound each family uses, and which OS voice speaks (Part 19).
struct SpeechSettingsView: View {
    @Environment(\.theme) var theme
    let model: MonitorModel

    static let kinds: [(String, String)] = [
        ("needs_you", "Needs a decision"), ("failed", "Task failed"), ("landing", "Landing frozen"),
        ("daemon_down", "Daemon down"), ("credentials", "Credentials failing"),
        ("provider_outage", "Provider outage"), ("stalled", "Pipeline stalled"), ("landed", "Landed"), ("review", "Pipeline review"),
        ("disk", "Disk nearly full"), ("environment", "Environment problem"), ("blocked", "Task blocked"), ("host_lost", "Host lost"), ("runtime", "Runtime update failed"), ("definition", "Definition refused"), ("tracker", "Tracker problem"), ("provider_limit", "Provider limit"), ("provider_overage", "Provider overage"), ("provider_budget", "Provider budget"),
    ]
    static let sounds = ["Glass", "Basso", "Tink", "Ping", "Pop", "Submarine", "Sosumi", "Funk"]

    var body: some View {
        let _ = model.announcerRevision
        let a = model.announcer
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text("Sound and speech").font(theme.body(.semibold))
            ForEach(Self.kinds, id: \.0) { kind, label in
                Toggle(label, isOn: Binding(
                    get: { a.settings.kinds.contains(kind) },
                    set: { on in
                        if on { a.settings.kinds.insert(kind) } else { a.settings.kinds.remove(kind) }
                        model.announcerSettingsChanged()
                    }
                ))
            }
            Divider()
            ForEach([("decision", "Wants a decision"), ("broke", "Broke"), ("info", "Other")], id: \.0) { family, label in
                Picker(label, selection: Binding(
                    get: { a.settings.sounds[family] ?? "Tink" },
                    set: { a.settings.sounds[family] = $0; model.announcerSettingsChanged() }
                )) { ForEach(Self.sounds, id: \.self) { Text($0).tag($0) } }
            }
            Picker("Voice", selection: Binding(
                get: { a.settings.voice },
                set: { a.settings.voice = $0; model.announcerSettingsChanged() }
            )) {
                Text("System default").tag("")
                ForEach(a.availableVoices(), id: \.self) { Text($0).tag($0) }
            }
            if let line = a.noChannelLine {
                Text(line).font(theme.small()).foregroundStyle(theme.muted)
            }
            Text(a.isSpeaker ? "This monitor is the one that speaks." : "Another monitor speaks, or none is needed.")
                .font(theme.small()).foregroundStyle(theme.muted)
            HStack(spacing: theme.spacing) {
                Text("Volume")
                Slider(value: Binding(
                    get: { a.settings.volume },
                    set: { a.settings.volume = min(1, max(0, $0)); model.announcerSettingsChanged() }
                ), in: 0...1)
                .accessibilityIdentifier("speech-volume")
                Text("\(Int((a.settings.volume * 100).rounded()))%")
                    .font(theme.small()).foregroundStyle(theme.muted)
                    .frame(width: 36, alignment: .trailing)
                Button("Test") { a.test() }
                    .accessibilityIdentifier("speech-test")
            }
        }
        .padding(theme.spacingLarge)
        .frame(width: 320)
    }
}
