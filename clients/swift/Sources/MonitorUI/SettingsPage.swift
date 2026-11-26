// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// Settings (s430): proposal policy, notification routes with a Test button, schedules with
/// Run now, and the tracker. Every control sends one small change to the daemon, which writes
/// it to the file and applies it without a restart. A secret is never shown or typed here.
struct SettingsPage: View {
    @Environment(\.theme) var theme
    let model: MonitorModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                Text("Settings").font(theme.title())
                if let n = model.settingsNotice {
                    Text(n).font(theme.body()).foregroundStyle(theme.muted)
                        .accessibilityIdentifier("settings:notice")
                }
                if let s = model.settings {
                    proposals(s)
                    schedules(s)
                    tracker(s)
                    placement(s)
                } else {
                    Text("Loading the project's settings.").font(theme.body()).foregroundStyle(theme.muted)
                }
                if let n = model.notifySettings { routes(n) }
                DevicesSection(model: model)
            }.padding(theme.spacingLarge)
        }
        .onAppear { model.loadSettings(); model.loadDevices() }
    }

    func choice(_ title: String, _ id: String, value: String, options: [String], _ pick: @escaping (String) -> Void) -> some View {
        Picker(title, selection: Binding(get: { value }, set: pick)) {
            ForEach(options, id: \.self) { Text($0).tag($0) }
        }
        .font(theme.body())
        .accessibilityIdentifier(id)
    }

    func proposals(_ s: SettingsSnapshot) -> some View {
        section("Proposal policy (\(s.project))", theme: theme) {
            ForEach(SettingsSnapshot.dispositionKeys, id: \.self) { k in
                choice(k, "settings:\(k)", value: s.dispositions[k] ?? "proposed", options: SettingsSnapshot.dispositions) {
                    model.changeProjectSetting(SettingsPayload.disposition(key: k, value: $0))
                }
            }
            Text("Introspection findings, by severity and cost").font(theme.small()).foregroundStyle(theme.muted)
            ForEach(SettingsSnapshot.introspectionKeys, id: \.self) { k in
                choice(k, "settings:introspection:\(k)", value: s.introspection[k] ?? "proposed", options: SettingsSnapshot.dispositions) {
                    model.changeProjectSetting(SettingsPayload.introspection(key: k, value: $0))
                }
            }
            Stepper("At most \(s.perTask) per task", value: Binding(get: { s.perTask }, set: { model.changeProjectSetting(SettingsPayload.cap(key: "per_task", value: max(1, $0))) }), in: 1...50)
                .font(theme.body())
            Stepper("At most \(s.perDay) per day", value: Binding(get: { s.perDay }, set: { model.changeProjectSetting(SettingsPayload.cap(key: "per_day", value: max(1, $0))) }), in: 1...500)
                .font(theme.body())
        }
    }

    func schedules(_ s: SettingsSnapshot) -> some View {
        section("Recurring jobs", theme: theme) {
            if s.schedules.isEmpty { Text("None. Add one to the project file.").font(theme.body()).foregroundStyle(theme.muted) }
            ForEach(s.schedules, id: \.name) { j in
                ScheduleRow(model: model, job: j)
            }
        }
    }

    func tracker(_ s: SettingsSnapshot) -> some View {
        section("Issue tracker", theme: theme) {
            if let t = s.tracker {
                Stepper("Look every \(t.intervalS) s", value: Binding(get: { t.intervalS }, set: { model.changeProjectSetting(SettingsPayload.tracker(key: "interval_s", value: max(1, $0))) }), in: 5...3600, step: 5)
                    .font(theme.body())
                choice("import as", "settings:tracker:import_as", value: t.importAs, options: SettingsSnapshot.dispositions) {
                    model.changeProjectSetting(SettingsPayload.tracker(key: "import_as", value: $0))
                }
                Toggle("Only issues with the label \"\(t.label)\"", isOn: Binding(get: { t.requireLabel }, set: { model.changeProjectSetting(SettingsPayload.tracker(key: "require_label", value: $0)) }))
                    .font(theme.body())
                if !t.token.isEmpty { Text("Token: \(t.token)").font(theme.small()).foregroundStyle(theme.muted) }
            } else {
                Text("No tracker is configured for this project.").font(theme.body()).foregroundStyle(theme.muted)
            }
        }
    }

    func placement(_ s: SettingsSnapshot) -> some View {
        section("Where runs go", theme: theme) {
            if let r = model.remoteSettings {
                RemotePlacementBody(remote: r, project: s.project,
                                    pickGlobal: { model.changeGlobalMode($0) },
                                    pickProject: { model.changeProjectMode($0) })
            } else {
                Text("Loading where runs go.").font(theme.body()).foregroundStyle(theme.muted)
            }
            NodeAcceptToggle(model: model)
            NodesPoller(model: model)
        }
    }

    func routes(_ n: NotifySettings) -> some View {
        section("Notifications", theme: theme) {
            ForEach(n.routes, id: \.condition) { r in
                VStack(alignment: .leading, spacing: theme.spacingSmall) {
                    Text(r.condition).font(theme.body(.semibold))
                    HStack(spacing: theme.spacing) {
                        ForEach(n.channels, id: \.self) { ch in
                            Toggle(ch, isOn: Binding(
                                get: { n.sends(r.condition, to: ch) },
                                set: { model.changeNotifySetting(SettingsPayload.route(condition: r.condition, channel: ch, delay: $0 ? "0" : nil)) }))
                                .toggleStyle(.checkbox).font(theme.small())
                                .accessibilityIdentifier("settings:route:\(r.condition):\(ch)")
                        }
                    }
                }
            }
            HStack(spacing: theme.spacing) {
                ForEach(n.channels.filter { $0 != "monitor" }, id: \.self) { ch in
                    Button("Test \(ch)") { model.testNotification(channel: ch) }
                        .accessibilityIdentifier("settings:test:\(ch)")
                }
            }
        }
    }
}

struct ScheduleRow: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let job: SettingsSnapshot.Schedule
    @State private var every = ""

    var body: some View {
        HStack(spacing: theme.spacing) {
            Text("\(job.name) (\(job.kind))").font(theme.body())
            TextField("every", text: $every)
                .onSubmit { model.changeProjectSetting(SettingsPayload.schedule(name: job.name, every: every)) }
                .frame(width: 90)
                .accessibilityIdentifier("settings:every:\(job.name)")
            Button("Run now") { model.runSchedule(job.name) }
                .accessibilityIdentifier("settings:run:\(job.name)")
        }
        .onAppear { every = job.every }
    }
}

/// The body of the "Where runs go" section (#891): the global mode and the project's.
struct RemotePlacementBody: View {
    @Environment(\.theme) var theme
    let remote: RemoteSettings
    let project: String
    let pickGlobal: (String) -> Void
    let pickProject: (String) -> Void

    private func choice(_ title: String, _ id: String, value: String, enabled: Bool, _ pick: @escaping (String) -> Void) -> some View {
        Picker(title, selection: Binding(get: { value }, set: pick)) {
            ForEach(RemoteSettings.modes, id: \.self) { Text($0).tag($0) }
        }
        .font(theme.body())
        .disabled(!enabled)
        .accessibilityIdentifier(id)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            choice("All of MASON", "settings:remote:global", value: remote.mode, enabled: true, pickGlobal)
            if let p = remote.problem { Text(p).font(theme.small()).foregroundStyle(theme.muted) }
            if let c = remote.changedText { Text(c).font(theme.small()).foregroundStyle(theme.muted) }
            if let p = remote.project(project) {
                choice(project, "settings:remote:project", value: p.mode, enabled: true, pickProject)
                Text(p.sourceText).font(theme.small()).foregroundStyle(theme.muted)
                if !p.runsOn.isEmpty { Text("GitHub runner: \(p.runsOn)").font(theme.small()).foregroundStyle(theme.muted) }
            }
            Text(RemoteSettings.explanation).font(theme.small()).foregroundStyle(theme.muted)
        }
    }
}
