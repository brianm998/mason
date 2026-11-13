// SPDX-License-Identifier: GPL-3.0-or-later

import MasonMascot
import MasonProtocol
import MasonTokens
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

func section<Content: View>(_ title: String, theme: Theme, state: String? = nil, @ViewBuilder _ content: () -> Content) -> some View {
    VStack(alignment: .leading, spacing: theme.spacing) {
        HStack(spacing: theme.spacingSmall) {
            Text(title).font(theme.title()).foregroundStyle(theme.text)
            if let state { StateIcon(state: state) }
        }
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
    @State private var adding = false

    var statusLine: some View {
        Text(model.connected ? (model.home?.status ?? "…") : "Not connected")
            .font(theme.title()).foregroundStyle(model.attention > 0 ? theme.attention : theme.text)
            .accessibilityIdentifier("status")
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                let pic = model.homeMascot
                if pic.large {
                    VStack(spacing: theme.spacing) {
                        MascotPicture(id: pic.id, points: MasonTokens.sizeMascotEmpty)
                        statusLine
                        Button("Add task") { adding = true }.buttonStyle(.bordered)
                            .accessibilityIdentifier("home:add-task")
                    }.frame(maxWidth: .infinity)
                } else {
                    HStack(spacing: theme.spacingLarge) {
                        MascotPicture(id: pic.id, points: MasonTokens.sizeMascotStatus)
                        statusLine
                    }
                }

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
                                Text(a.display.isEmpty ? a.model : a.display).font(theme.body(.semibold))
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

                if let h = model.home, h.hasCompilation {
                    CompilationSection(model: model, c: h.compilation)
                }

                ForEach(model.home?.stages ?? [], id: \.state) { st in
                    section(st.title, theme: theme, state: st.state) {
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
        .sheet(isPresented: $adding) { AddTaskSheet(model: model, isPresented: $adding) }
    }
}

/// A mascot picture: decorative, sized in points that scale with the text zoom, never tinted.
/// An id the art does not have draws nothing.
struct MascotPicture: View {
    @Environment(\.theme) var theme
    let id: String
    let points: Int

    var body: some View {
        if let img = Mascot.illustration(id) {
            let side = CGFloat(Double(points) * theme.scale)
            img.resizable().scaledToFit().frame(width: side, height: side)
                .accessibilityHidden(true)
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

/// Home's Compilation section: what holds a `compile` slot, what waits for one
/// and why (in line order), the CPU reading, and the limit's stepper.
struct CompilationSection: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let c: Mason_V1_Compilation

    private func age(_ since: UInt64) -> String {
        let now = model.daemonNow()
        return Format.duration(ms: now > since ? now - since : 0)
    }

    @ViewBuilder private func who(_ label: String, task: UInt64) -> some View {
        if task > 0 {
            Button("#\(task)") { model.go(.task(task)) }.buttonStyle(.link)
        } else {
            Text(label).lineLimit(1).truncationMode(.middle).help(label)
        }
    }

    var body: some View {
        section("Compilation", theme: theme) {
            HStack(spacing: theme.spacing) {
                Text("Compiling: \(c.used) of at most")
                Button { model.setCompileLimit(c.limit > 1 ? c.limit - 1 : 1) } label: { Image(systemName: "minus") }
                    .disabled(c.limit <= 1)
                    .accessibilityIdentifier("compile:decrease")
                Text("\(c.limit)").font(theme.body(.semibold))
                Button { model.setCompileLimit(c.limit + 1) } label: { Image(systemName: "plus") }
                    .accessibilityIdentifier("compile:increase")
                Text(c.cpuLine).foregroundStyle(theme.muted)
            }
            .font(theme.body())
            .buttonStyle(.bordered)
            .help("Lowering the limit never stops a compile that is running; it holds back new ones until the count falls.")
            .accessibilityIdentifier("compile")

            if c.running.isEmpty {
                Text("Nothing is compiling.").font(theme.body()).foregroundStyle(theme.muted)
            }
            ForEach(Array(c.running.enumerated()), id: \.offset) { i, r in
                HStack(spacing: theme.spacing) {
                    who(r.label, task: r.task)
                    if r.interactive { Text("interactive").foregroundStyle(theme.muted) }
                    Text("for \(age(r.sinceMs))").foregroundStyle(theme.muted)
                    if r.idleMs >= 60_000 {
                        Text("no CPU for \(Format.duration(ms: r.idleMs))").foregroundStyle(theme.slow)
                    }
                }
                .font(theme.body())
                .accessibilityIdentifier("compile:running:\(i)")
            }

            if !c.pending.isEmpty {
                Text("Waiting, in line order").font(theme.body(.semibold)).padding(.top, theme.spacingSmall)
            }
            ForEach(c.pending, id: \.position) { w in
                HStack(alignment: .firstTextBaseline, spacing: theme.spacing) {
                    Text("\(w.position).").foregroundStyle(theme.muted)
                    who(w.label, task: w.task)
                    Text(w.reason)
                    Text(w.interactive ? "interactive, \(age(w.sinceMs))" : w.landing ? "landing, \(age(w.sinceMs))" : "\(w.stepsRemaining) steps from landing, \(age(w.sinceMs))")
                        .font(theme.small()).foregroundStyle(theme.muted)
                }
                .font(theme.body())
                .accessibilityIdentifier("compile:pending:\(w.position)")
            }

            ForEach(Array(c.notes.enumerated()), id: \.offset) { i, n in
                Text(n).font(theme.small()).foregroundStyle(theme.attention)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("compile:note:\(i)")
            }
            Text(c.rules).font(theme.small()).foregroundStyle(theme.muted)
                .fixedSize(horizontal: false, vertical: true)
        }
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
            HStack(alignment: .top, spacing: theme.spacingLarge) {
            if let art = Mascot.illustration(r.state) {
                art.resizable().scaledToFit()
                    .frame(width: theme.illustration, height: theme.illustration)
                    .accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                Text("\(r.key) \(r.title)").font(theme.title()).accessibilityIdentifier("detail:\(r.id)")
                Text("\(r.project), \(r.state.replacingOccurrences(of: "_", with: " ")), \(Format.duration(ms: r.inStateMs)) in this state")
                    .font(theme.body()).foregroundStyle(theme.muted)
                if !r.whyWaiting.isEmpty {
                    LinkedText(model: model, text: r.whyWaiting).accessibilityIdentifier("detail:why-waiting")
                }
                if r.issue != 0 || !d.ciLinks.isEmpty {
                    CiLinksLine(links: d.ciLinks, issue: r.issue, issueURL: r.issueURL)
                }
                HStack(spacing: theme.spacingLarge) {
                    CopyButton(label: r.key, value: r.key)
                    if !d.branch.isEmpty { CopyButton(label: "branch", value: d.branch) }
                    if !d.worktree.isEmpty { CopyButton(label: "worktree", value: d.worktree) }
                }
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
            BlockersView(model: model, row: r)
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
            if d.hasProposal { ProposalView(model: model, row: r, info: d.proposal, title: r.title) }
            if !d.failure.isEmpty {
                LinkedText(model: model, text: "Failed: \(d.failure)", color: theme.attention)
                    .accessibilityIdentifier("detail:failure")
            }

            if !d.stepTokens.isEmpty {
                section("Tokens", theme: theme) {
                    ForEach(Array(d.stepTokens.enumerated()), id: \.offset) { _, l in
                        Text(tokenLineText(l)).font(theme.body())
                    }
                }
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
                    // Every session this run started, so a retry up the ladder shows.
                    ForEach(Array(s.launches.enumerated()), id: \.offset) { _, l in
                        LinkedText(model: model, text: "\(l.kind): \(l.display)", font: theme.small(), color: theme.muted)
                            .padding(.leading, theme.spacingLarge)
                    }
                }
            }

            if !d.closing.isEmpty {
                section("Tests", theme: theme) { LinkedText(model: model, text: d.closing).accessibilityIdentifier("detail:closing") }
            }
            if !d.debts.isEmpty {
                section("Debts", theme: theme) {
                    ForEach(d.debts, id: \.id) { x in
                        // The debt's own `#id` is not a task, so only the rest is linked.
                        HStack(spacing: 0) {
                            Text("#\(x.id) ").font(theme.body())
                            LinkedText(model: model, text: "\(x.scope), \(x.status)\(x.deferred ? ", runs after landing" : "")")
                        }
                    }
                }
            }
            if !d.plan.isEmpty || d.hasPlanView {
                section("Plan", theme: theme) { PlanSection(model: model, detail: d) }
            }
            if !d.body.isEmpty {
                section("Task", theme: theme) { LinkedText(model: model, text: d.body, selectable: true).accessibilityIdentifier("detail:body") }
            }
            if !model.liveLog.isEmpty {
                section("Log", theme: theme) {
                    LinkedText(model: model, text: model.liveLog.joined(separator: "\n"), font: theme.mono(), selectable: true)
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
                        LinkedText(model: model, text: "Frozen: \(f.project): \(f.reason)\(f.fixTask > 0 ? " (fix: task #\(f.fixTask))" : "")",
                                   color: theme.attention)
                    }
                    section("Queue", theme: theme) {
                        if l.queue.isEmpty {
                            MascotPicture(id: "nothing_landing", points: MasonTokens.sizeMascotEmpty)
                            Text("Nothing is landing.").font(theme.body()).foregroundStyle(theme.muted)
                        }
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
                            HStack(spacing: 0) {
                                Text("#\(x.id) ").font(theme.body())
                                LinkedText(model: model, text: "task #\(x.task) \(x.scope): \(x.reason.isEmpty ? "no reason given" : x.reason)")
                            }
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
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                Text("LLM usage").font(theme.title())
                ForEach(model.usage?.providers ?? [], id: \.name) { p in
                    UsageAccount(p: p)
                }
                if let n = model.usage?.note { Text(n).font(theme.body()).foregroundStyle(theme.muted) }
                SessionStepper(model: model)
            }
            .padding(theme.spacingLarge)
        }
    }
}

/// The text of a window's percentage, worded as the conformance runner words it.
func windowPercentText(_ w: Mason_V1_UsageWindow) -> String {
    if !w.reported { return "not reported yet" }
    if w.hasPercent_p { return "\(Int(w.percent.rounded()))%" }
    return "no percentage reported"
}

func tokenLineText(_ l: Mason_V1_TokenLine) -> String {
    var parts: [String] = []
    if l.hasInput_p { parts.append("in \(countText(l.input))") }
    if l.hasOutput_p { parts.append("out \(countText(l.output))") }
    if l.hasCacheRead_p { parts.append("cache read \(countText(l.cacheRead))") }
    if l.hasCacheWrite_p { parts.append("cache write \(countText(l.cacheWrite))") }
    if l.hasCost_p { parts.append(String(format: "$%.2f", l.costUsd)) }
    return "\(l.name): " + (parts.isEmpty ? "nothing reported" : parts.joined(separator: ", ")) + " (\(l.age))"
}

func countText(_ n: UInt64) -> String {
    if n < 1000 { return String(n) }
    if n < 1_000_000 { return String(format: "%.1fk", Double(n) / 1e3) }
    return String(format: "%.1fM", Double(n) / 1e6)
}

/// One provider account: its slots, each window as a bar with its reset, overage and
/// blocked text, spend for an api account, and tokens. Every figure carries its age.
struct UsageAccount: View {
    @Environment(\.theme) var theme
    let p: Mason_V1_ProviderStatus

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            Text("\(p.name) (\(p.adapter), \(p.billing)): \(p.status), \(p.sessionsUsed) of \(p.sessionsLimit) sessions")
                .font(theme.body(.semibold))
                .accessibilityIdentifier("usage:\(p.name)")
            Text("warn at \(Int(p.warnAt * 100))%, overage \(p.overage)" + (p.monthlyBudgetUsd > 0 ? ", budget $\(Int(p.monthlyBudgetUsd))/month" : ""))
                .font(theme.small()).foregroundStyle(theme.muted)
            Text(p.hasSnapshot_p ? p.age : "nothing reported yet").font(theme.small()).foregroundStyle(theme.muted)
            ForEach(p.windows, id: \.id) { w in
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(w.label): \(windowPercentText(w))" + (w.reported ? ", \(w.state)" : ""))
                        .font(theme.body())
                    if w.reported && w.hasPercent_p {
                        ProgressView(value: min(max(w.percent / 100, 0), 1))
                            .tint(w.state == "blocked" ? theme.attention : nil)
                    }
                    if w.reported {
                        Text(w.hasReset_p ? w.resetsText : "no reset time reported").font(theme.small()).foregroundStyle(theme.muted)
                        if !w.trend.isEmpty { Text(w.trend).font(theme.small()).foregroundStyle(theme.muted) }
                        Text(w.age).font(theme.small()).foregroundStyle(theme.muted)
                    }
                }
                .accessibilityElement(children: .combine)
                .accessibilityIdentifier("usage-window:\(p.name):\(w.id)")
            }
            if p.hasSpend {
                let s = p.spend
                if s.hasSpend_p {
                    Text(String(format: "Spend: $%.2f today, $%.2f this week, $%.2f this month", s.todayUsd, s.weekUsd, s.monthUsd)
                        + (s.hasBudget_p ? String(format: " of $%.2f budget", s.budgetUsd) : "") + " (\(s.age))")
                        .font(theme.body())
                    if s.hasBudget_p && s.budgetUsd > 0 {
                        ProgressView(value: min(max(s.monthUsd / s.budgetUsd, 0), 1))
                    }
                } else {
                    Text("Spend: nothing reported yet").font(theme.body()).foregroundStyle(theme.muted)
                }
            }
            if !p.overageText.isEmpty { Text(p.overageText).font(theme.body()) }
            if !p.blockedText.isEmpty { Text(p.blockedText).font(theme.body()).foregroundStyle(theme.attention) }
            tokens(p.tokensWindow)
            tokens(p.tokensWeek)
            Text("Doing: \(p.doing)").font(theme.small()).foregroundStyle(theme.muted)
        }
    }

    @ViewBuilder
    func tokens(_ t: Mason_V1_UsageTokens) -> some View {
        if !(t.byProject.isEmpty && t.byModel.isEmpty && t.topTasks.isEmpty) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Tokens, \(t.period)").font(theme.body(.semibold))
                ForEach(Array(t.byProject.enumerated()), id: \.offset) { _, l in Text("project " + tokenLineText(l)).font(theme.small()) }
                ForEach(Array(t.byModel.enumerated()), id: \.offset) { _, l in Text("model " + tokenLineText(l)).font(theme.small()) }
                ForEach(Array(t.topTasks.enumerated()), id: \.offset) { _, l in Text("task " + tokenLineText(l)).font(theme.small()) }
            }
        }
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
                    Text(h.cpuLine).font(theme.body())
                    section("Waiting", theme: theme) {
                        if h.waiting.isEmpty { Text("Nothing is waiting.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(h.waiting, id: \.self) { Text($0).font(theme.body()) }
                    }
                }
                if let i = model.introspection {
                    section("Findings", theme: theme) {
                        if i.findings.isEmpty {
                            MascotPicture(id: "all_clear", points: MasonTokens.sizeMascotStatus)
                            Text("Nothing found yet.").font(theme.body()).foregroundStyle(theme.muted)
                        }
                        ForEach(i.findings, id: \.id) { f in
                            VStack(alignment: .leading) {
                                Text("#\(f.id) [\(f.verdict)] \(f.title) (proposal #\(f.task), metric \(f.metric))").font(theme.body())
                                if !f.text.isEmpty { Text(f.text).font(theme.body()).foregroundStyle(theme.muted) }
                            }
                        }
                    }
                    section("Recurring jobs", theme: theme) {
                        ForEach(i.jobs, id: \.name) { j in
                            Text("\(j.name) (\(j.kind)): \(j.runs) run(s). \(j.status)").font(theme.body())
                        }
                    }
                    section("Digest (last 7 days)", theme: theme) {
                        ForEach(i.digest, id: \.self) { Text($0).font(theme.body()) }
                    }
                    section("Triage", theme: theme) {
                        Text(i.triageOn ? "On. \(i.triageWaiting) suggestion(s) wait for your review; triage never answers a question." : "Off.")
                            .font(theme.body())
                    }
                }
            }.padding(theme.spacingLarge)
        }
    }
}
