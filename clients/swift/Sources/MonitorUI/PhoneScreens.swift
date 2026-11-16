// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// The page a row of the phone's sidebar pushed. The pages are the Mac monitor's own, laid out in
/// one narrow column (`compact`), so a page added there reaches the phone unless it is in
/// `PhoneLayout.phoneHasNo`.
struct PhoneDetailScreen: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let page: Page

    var body: some View {
        Group {
            switch page {
            case .home: HomePage(model: model, compact: true)
            case .task(let id): TaskDetailPage(model: model, id: id, compact: true)
            case .landing: LandingPage(model: model)
            case .usage: UsagePage(model: model, compact: true)
            case .health: HealthPage(model: model)
            case .settings: SettingsPage(model: model)
            case .section(let name): SectionPage(model: model, name: name, compact: true)
            case .pipeline:
                // Graph editing is not on a phone (`PhoneLayout.phoneHasNo`).
                Text("The pipeline graph is edited in the Mac monitor.")
                    .font(theme.body()).foregroundStyle(theme.muted).padding(theme.spacingLarge)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(theme.background)
    }
}

/// "Sessions (all projects): 3 of 4 [-] 4 [+]" for a narrow screen: the words on one line, the
/// buttons under them. It sends what the Mac's stepper sends (`MonitorModel.stepSessionLimit`).
struct PhoneSessionStepper: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        let used = model.home?.sessionsUsed ?? 0
        let limit = model.home?.sessionsLimit ?? 0
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            Text("Sessions (all projects): \(used) of \(limit)").font(theme.body())
            HStack(spacing: theme.spacing) {
                Button { model.stepSessionLimit(by: -1) } label: { Image(systemName: "minus") }
                    .accessibilityLabel("Fewer sessions")
                    .accessibilityIdentifier("sessions:decrease")
                Text("\(limit)").font(theme.body(.semibold))
                Button { model.stepSessionLimit(by: 1) } label: { Image(systemName: "plus") }
                    .accessibilityLabel("More sessions")
                    .accessibilityIdentifier("sessions:increase")
            }
            .buttonStyle(.bordered)
            Text("Lowering the limit never stops anything running; it holds back new work until the count falls.")
                .font(theme.small()).foregroundStyle(theme.muted)
        }
        .accessibilityIdentifier("sessions")
    }
}
