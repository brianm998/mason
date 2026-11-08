// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import MonitorCore
import SwiftUI

/// The main view: exactly one of Home, a task's detail, or another page.
struct MainPane: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        Group {
            switch model.page {
            case .home: HomePage(model: model)
            case .task(let id): TaskDetailPage(model: model, id: id)
            case .landing: LandingPage(model: model)
            case .usage: UsagePage(model: model)
            case .pipeline: EditorPage(model: model)
            case .health: HealthPage(model: model)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(theme.background)
    }
}

func section<Content: View>(_ title: String, theme: Theme, @ViewBuilder _ content: () -> Content) -> some View {
    VStack(alignment: .leading, spacing: theme.spacing) {
        Text(title).font(theme.title()).foregroundStyle(theme.text)
        content()
    }
}

/// Home: the overall status, the attention queue, the active models with
/// their task and step, the session-limit stepper, the landing queue, and
/// detail for the tasks in each stage. (Counts and task lists live in the top
/// bar and the sidebar, not here.)
struct HomePage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                Text(model.connected ? (model.home?.status ?? "…") : "Not connected")
                    .font(theme.title()).foregroundStyle(model.attention > 0 ? theme.attention : theme.text)
                    .accessibilityIdentifier("status")

                if let attention = model.home?.attention, !attention.isEmpty {
                    section("Needs you", theme: theme) {
                        ForEach(attention, id: \.task.id) { a in
                            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                                Button { model.go(.task(a.task.id)) } label: {
                                    Text("\(a.task.project) \(a.task.key): \(a.task.title)")
                                        .font(theme.body(.semibold)).foregroundStyle(theme.muted)
                                }.buttonStyle(.plain)
                                if a.hasCard { DecisionCardView(model: model, task: a.task.id, card: a.card) }
                                if a.hasApproval { ApprovalView(model: model, task: a.task.id, approval: a.approval) }
                            }
                        }
                    }
                }

                HStack(alignment: .top, spacing: theme.spacingLarge * 2) {
                    section("Active models", theme: theme) {
                        let active = model.home?.active ?? []
                        if active.isEmpty { Text("Nothing is running.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(active, id: \.task) { a in
                            HStack(spacing: theme.spacing) {
                                Text(a.model).font(theme.body(.semibold))
                                Button("\(a.project) #\(a.task)") { model.go(.task(a.task)) }.buttonStyle(.link)
                                Text(a.step).foregroundStyle(theme.muted)
                            }
                            .font(theme.body())
                            .accessibilityIdentifier("active:\(a.task)")
                        }
                        SessionStepper(model: model)
                    }
                    section("Landing queue", theme: theme) {
                        let queue = model.landing?.queue ?? model.home?.stages.first { $0.state == "landing" }?.rows ?? []
                        if queue.isEmpty { Text("Nothing is landing.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(queue, id: \.id) { r in
                            Button("\(r.key) \(r.title)") { model.go(.task(r.id)) }.buttonStyle(.link).font(theme.body())
                        }
                    }
                }

                ForEach(model.home?.stages ?? [], id: \.state) { st in
                    section(st.title, theme: theme) {
                        ForEach(st.rows, id: \.id) { r in
                            Button { model.go(.task(r.id)) } label: {
                                HStack(spacing: theme.spacing) {
                                    ProgressCells(model: model, cells: r.progress, slow: r.slow)
                                    Text("\(r.project) \(r.key)").font(theme.body(.semibold))
                                    Text(r.title).font(theme.body())
                                    Text(r.whyWaiting.isEmpty ? Format.duration(ms: r.inStateMs) : "\(r.whyWaiting), \(Format.duration(ms: r.inStateMs))")
                                        .font(theme.small()).foregroundStyle(r.slow ? theme.slow : theme.muted)
                                }
                            }.buttonStyle(.plain)
                        }
                    }
                }
            }
            .padding(theme.spacingLarge)
        }
    }
}

/// "Active models: 3 of [-] 4 [+]": changes how many LLM sessions may be held.
struct SessionStepper: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        let used = model.home?.sessionsUsed ?? 0
        let limit = model.home?.sessionsLimit ?? 0
        HStack(spacing: theme.spacing) {
            Text("Sessions: \(used) of")
            Button { model.setSessionLimit(limit > 0 ? limit - 1 : 0) } label: { Image(systemName: "minus") }
                .accessibilityIdentifier("sessions:decrease")
            Text("\(limit)").font(theme.body(.semibold))
            Button { model.setSessionLimit(limit + 1) } label: { Image(systemName: "plus") }
                .accessibilityIdentifier("sessions:increase")
        }
        .font(theme.body())
        .buttonStyle(.bordered)
        .help("Lowering the limit never stops anything running; it holds back new work until the count falls.")
        .accessibilityIdentifier("sessions")
    }
}

/// A task's page: step progress, activity, the live log, the plan, checks,
/// the decision card and the actions; copy buttons; why exploring was skipped.
struct TaskDetailPage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let id: UInt64

    var body: some View {
        ScrollView {
            if let d = model.taskDetail, d.hasRow, d.row.id == id {
                content(d)
            } else {
                Text(model.connected ? "Loading task #\(id)…" : "Not connected")
                    .font(theme.body()).foregroundStyle(theme.muted).padding(theme.spacingLarge)
            }
        }
    }

    @ViewBuilder func content(_ d: TaskDetailView) -> some View {
        let r = d.row
        VStack(alignment: .leading, spacing: theme.spacingLarge) {
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                Text("\(r.key) \(r.title)").font(theme.title()).accessibilityIdentifier("detail:\(r.id)")
                Text("\(r.project), \(r.state.replacingOccurrences(of: "_", with: " ")), \(Format.duration(ms: r.inStateMs)) in this state")
                    .font(theme.body()).foregroundStyle(theme.muted)
                if !r.whyWaiting.isEmpty {
                    Text(r.whyWaiting).font(theme.body()).foregroundStyle(theme.text)
                }
                HStack(spacing: theme.spacingLarge) {
                    CopyButton(label: r.key, value: r.key)
                    if !d.branch.isEmpty { CopyButton(label: "branch", value: d.branch) }
                    if !d.worktree.isEmpty { CopyButton(label: "worktree", value: d.worktree) }
                }
            }

            HStack(spacing: theme.spacing) {
                ProgressCells(model: model, cells: r.progress, slow: r.slow)
                if !r.typical.isEmpty { Text(r.typical).font(theme.small()).foregroundStyle(theme.muted) }
            }
            if let c = r.progress.running { ProgressDetail(model: model, cell: c) }

            ActionButtons(model: model, row: r, hasApprovalView: d.hasApproval)

            if !r.blockers.isEmpty {
                HStack(spacing: theme.spacingSmall) {
                    Text("waiting for").font(theme.body()).foregroundStyle(theme.muted)
                    ForEach(r.blockers, id: \.self) { b in
                        Button("#\(b)") { model.go(.task(b)) }
                            .buttonStyle(.link).font(theme.body())
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("waiting for \(r.blockers.map { "#\($0)" }.joined(separator: ", "))")
                .accessibilityIdentifier("waiting-for")
            }
            if !r.blocks.isEmpty {
                HStack(spacing: theme.spacingSmall) {
                    Text("blocks").font(theme.body()).foregroundStyle(theme.muted)
                    ForEach(r.blocks, id: \.self) { b in
                        Button("#\(b)") { model.go(.task(b)) }
                            .buttonStyle(.link).font(theme.body())
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityLabel("blocks \(r.blocks.map { "#\($0)" }.joined(separator: ", "))")
                .accessibilityIdentifier("blocks")
            }

            if !d.route.isEmpty {
                HStack(spacing: theme.spacing) {
                    Text("Started at \(d.startedAt): \(d.route).").font(theme.body()).foregroundStyle(theme.muted)
                    if r.allowedActions.contains("explore_anyway") {
                        Text("Use “Explore anyway” to explore it after all.").font(theme.small()).foregroundStyle(theme.muted)
                    }
                    if d.exploreReturns > 0 {
                        Text("Sent back to exploring \(d.exploreReturns) time(s), at no cost.").font(theme.small()).foregroundStyle(theme.muted)
                    }
                }
            }

            ForEach(d.numbers, id: \.kind) { n in
                Text("Number \(n.kind) \(n.number): \(n.path)\(n.source == "stage" ? " (its stage)" : "")\(n.released ? " (released, not reused)" : "")")
                    .font(theme.small()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("number:\(n.kind)")
            }

            if d.hasCard { DecisionCardView(model: model, task: r.id, card: d.card) }
            if d.hasApproval { ApprovalView(model: model, task: r.id, approval: d.approval) }
            if !d.failure.isEmpty {
                Text("Failed: \(d.failure)").font(theme.body()).foregroundStyle(theme.attention)
            }

            section("Steps", theme: theme) {
                ForEach(d.steps, id: \.run) { s in
                    HStack(spacing: theme.spacing) {
                        Text("run \(s.run)").foregroundStyle(theme.muted)
                        Text(s.name.isEmpty ? s.category : s.name).font(theme.body(.semibold))
                        Text("attempt \(s.attempt)").foregroundStyle(theme.muted)
                        Text(s.outcome)
                        if let c = r.progress.first(where: { $0.step == s.name && $0.state == .part && $0.run == s.run }) {
                            Text(c.countText).foregroundStyle(theme.muted)
                            Text(c.timeText(now: model.daemonNow())).foregroundStyle(c.isStale(now: model.daemonNow()) ? theme.slow : theme.muted)
                        }
                    }
                    .font(theme.body())
                    .accessibilityIdentifier("step:\(s.run)")
                }
            }

            if !d.closing.isEmpty {
                section("Tests", theme: theme) { Text(d.closing).font(theme.body()) }
            }
            if !d.debts.isEmpty {
                section("Debts", theme: theme) {
                    ForEach(d.debts, id: \.id) { x in
                        Text("#\(x.id) \(x.scope), \(x.status)\(x.deferred ? ", runs after landing" : "")")
                            .font(theme.body())
                    }
                }
            }
            if !d.plan.isEmpty || d.hasPlanView {
                section("Plan", theme: theme) { PlanSection(detail: d) }
            }
            if !d.body.isEmpty {
                section("Task", theme: theme) { Text(d.body).font(theme.body()).textSelection(.enabled) }
            }
            if !model.liveLog.isEmpty {
                section("Log", theme: theme) {
                    Text(model.liveLog.joined(separator: "\n"))
                        .font(theme.mono()).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(theme.spacing)
                        .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.surface))
                }
            }
        }
        .padding(theme.spacingLarge)
    }
}

struct LandingPage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                Text("Landing").font(theme.title())
                if let l = model.landing {
                    if !l.note.isEmpty {
                        Text(l.note).font(theme.body()).foregroundStyle(l.frozen ? theme.attention : theme.muted)
                    }
                    ForEach(l.modes, id: \.self) { m in
                        Text(m).font(theme.body()).foregroundStyle(theme.muted)
                    }
                    // One line per project: tracker off, polling with counts, or stopped with why.
                    ForEach(l.trackers, id: \.self) { t in
                        Text(t).font(theme.body()).foregroundStyle(t.contains("stopped") || t.contains(" off;") ? theme.attention : theme.muted)
                    }
                    ForEach(l.freezes, id: \.project) { f in
                        Text("Frozen: \(f.project): \(f.reason)\(f.fixTask > 0 ? " (fix: task #\(f.fixTask))" : "")")
                            .font(theme.body()).foregroundStyle(theme.attention)
                    }
                    section("Queue", theme: theme) {
                        if l.queue.isEmpty { Text("Nothing is landing.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(l.queue, id: \.id) { r in
                            Button("\(r.key) \(r.title)") { model.go(.task(r.id)) }.buttonStyle(.link)
                        }
                    }
                    section("Batches", theme: theme) {
                        if l.batches.isEmpty { Text("No batch has formed yet.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(l.batches, id: \.id) { b in
                            Text(batchText(b)).font(theme.body())
                        }
                        if l.queueIdleMs > 0 {
                            Text("Queue idle: \(l.queueIdleMs / 1000) s with tasks waiting and nothing landing.")
                                .font(theme.body()).foregroundStyle(theme.muted)
                        }
                    }
                    section("Debts", theme: theme) {
                        if l.debts.isEmpty { Text("Nothing is owed.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(l.debts, id: \.id) { x in
                            Text("#\(x.id) task #\(x.task) \(x.scope): \(x.reason.isEmpty ? "no reason given" : x.reason)")
                                .font(theme.body())
                        }
                    }
                }
            }.padding(theme.spacingLarge)
        }
    }
}

/// One line per batch: where it is, who landed, who was parked, and why.
func batchText(_ b: Mason_V1_BatchLine) -> String {
    func list(_ v: [UInt64]) -> String { v.map { "#\($0)" }.joined(separator: " ") }
    var t = "Batch #\(b.id) \(b.project) \(b.phase)\(b.running ? " (running)" : ""): \(list(b.tasks))"
    if !b.landed.isEmpty { t += ", landed \(list(b.landed))" }
    if !b.parked.isEmpty { t += ", parked \(list(b.parked))" }
    if !b.note.isEmpty { t += ". \(b.note)" }
    return t
}

struct UsagePage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacingLarge) {
            Text("LLM usage").font(theme.title())
            ForEach(model.usage?.providers ?? [], id: \.name) { p in
                Text("\(p.name) (\(p.adapter), \(p.billing)): \(p.status), \(p.sessionsUsed) of \(p.sessionsLimit) sessions in use; warn at \(Int(p.warnAt * 100))%, overage \(p.overage)" + (p.monthlyBudgetUsd > 0 ? ", budget $\(Int(p.monthlyBudgetUsd))/month" : "")).font(theme.body())
            }
            if let n = model.usage?.note { Text(n).font(theme.body()).foregroundStyle(theme.muted) }
            SessionStepper(model: model)
        }
        .padding(theme.spacingLarge)
    }
}

struct HealthPage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                Text("Health").font(theme.title())
                Text(model.connected ? "Connected to masond. Event log at #\(model.lastSeq)." : "Not connected.")
                    .font(theme.body())
                if let h = model.health {
                    section("Resources", theme: theme) {
                        ForEach(h.resources, id: \.name) { r in
                            Text("\(r.name) (\(r.kind)): \(r.used) of \(r.limit) in use").font(theme.body())
                        }
                    }
                    section("Waiting", theme: theme) {
                        if h.waiting.isEmpty { Text("Nothing is waiting.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(h.waiting, id: \.self) { Text($0).font(theme.body()) }
                    }
                }
            }.padding(theme.spacingLarge)
        }
    }
}
