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
            case .settings: SettingsPage(model: model)
            case .section(let name): SectionPage(model: model, name: name)
            }
        }
        .id(model.history.visit)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(theme.background)
    }
}

func section<Content: View>(_ title: String, theme: Theme, state: String? = nil, open: (() -> Void)? = nil, @ViewBuilder _ content: () -> Content) -> some View {
    VStack(alignment: .leading, spacing: theme.spacing) {
        HStack(spacing: theme.spacingSmall) {
            if let open {
                // The title and the '>' next to it both open the section's page.
                Button(action: open) { Text(title).font(theme.title()).foregroundStyle(theme.text) }
                    .buttonStyle(.plain).touchTarget().hint("Open the \(title) page")
                    .accessibilityIdentifier("section-title:\(title)")
            } else {
                Text(title).font(theme.title()).foregroundStyle(theme.text)
            }
            if let state { StateIcon(state: state) }
            if let open {
                Button(action: open) { Image(systemName: "chevron.right").font(theme.body(.semibold)).foregroundStyle(theme.muted) }
                    .buttonStyle(.plain).touchTarget().hint("Open the \(title) page")
                    .accessibilityLabel("Open \(title)")
                    .accessibilityIdentifier("section-open:\(title)")
            }
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
    /// A phone: the two side-by-side sections stack and each stage row is a column.
    var compact = false
    @State private var adding = false

    var statusLine: some View {
        Text(model.connected ? (model.home?.status ?? "…") : "Not connected")
            .font(theme.title()).foregroundStyle(model.attention > 0 ? theme.attention : theme.text)
            .accessibilityIdentifier("status")
    }

    @ViewBuilder var versionLine: some View {
        if !model.versionLine.isEmpty {
            Text(model.versionLine).font(theme.small()).foregroundStyle(theme.muted)
                .textSelection(.enabled).hint(model.versionTip)
                .accessibilityIdentifier("home:version")
        }
    }

    var body: some View {
        RestoringScroll(model: model, ready: model.home != nil) {
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
                versionLine

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

                let pair = compact
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: theme.spacingLarge))
                    : AnyLayout(HStackLayout(alignment: .top, spacing: theme.spacingLarge * 2))
                pair {
                    section("Active models", theme: theme, open: { model.go(.section("active_models")) }) {
                        let active = model.home?.active ?? []
                        if active.isEmpty { Text("Nothing is running.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(active, id: \.task) { a in
                            let line = compact
                                ? AnyLayout(VStackLayout(alignment: .leading, spacing: theme.spacingSmall))
                                : AnyLayout(HStackLayout(spacing: theme.spacing))
                            line {
                                Text(a.display.isEmpty ? a.model : a.display).font(theme.body(.semibold))
                                TaskLink(title: "\(a.project) #\(a.task)") { model.go(.task(a.task)) }
                                Text(a.step).foregroundStyle(theme.muted)
                            }
                            .font(theme.body())
                            .accessibilityIdentifier("active:\(a.task)")
                        }
                        if compact { PhoneSessionStepper(model: model) } else { SessionStepper(model: model) }
                    }
                    section("Landing queue", theme: theme, open: { model.go(.landing) }) {
                        let items = model.landing?.items ?? []
                        if !items.isEmpty {
                            LandingItems(model: model, items: items)
                        } else {
                            let queue = model.home?.stages.first { $0.state == "landing" }?.rows ?? []
                            if queue.isEmpty { Text("Nothing is landing.").font(theme.body()).foregroundStyle(theme.muted) }
                            ForEach(queue, id: \.id) { r in
                                TaskLink(title: "\(r.key) \(r.title)", font: theme.body()) { model.go(.task(r.id)) }
                            }
                        }
                    }
                }

                CpuSection(model: model, cpu: model.cpu, open: { model.go(.section("cpu")) })

                if let h = model.home, h.hasCompilation {
                    CompilationSection(model: model, c: h.compilation, open: { model.go(.section("compilation")) })
                }

                ForEach(model.home?.stages ?? [], id: \.state) { st in
                    section(st.title, theme: theme, state: st.state, open: Page.sectionPage(st.state).map { p in { model.go(p) } }) {
                        ForEach(st.rows, id: \.id) { r in
                            Button { model.go(.task(r.id)) } label: {
                                let line = compact
                                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: theme.spacingSmall))
                                    : AnyLayout(HStackLayout(spacing: theme.spacing))
                                line {
                                    ProgressCells(clock: model.live, cells: r.progress, slow: r.slow)
                                    Text("\(r.project) \(r.key)").font(theme.body(.semibold))
                                    Text(r.title).font(theme.body()).multilineTextAlignment(.leading)
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
            Text("Sessions (all projects): \(used) of")
            Button { model.stepSessionLimit(by: -1) } label: { Image(systemName: "minus") }.touchTarget()
                .accessibilityIdentifier("sessions:decrease")
            Text("\(limit)").font(theme.body(.semibold))
            Button { model.stepSessionLimit(by: 1) } label: { Image(systemName: "plus") }.touchTarget()
                .accessibilityIdentifier("sessions:increase")
        }
        .font(theme.body())
        .buttonStyle(.bordered)
        .hint("Lowering the limit never stops anything running; it holds back new work until the count falls.")
        .accessibilityIdentifier("sessions")
    }
}

/// Home's Compilation section: what holds a `compile` slot, what waits for one
/// and why (in line order), the CPU reading, and the limit's stepper.
struct CompilationSection: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let c: Mason_V1_Compilation
    /// Set on home: the title and its '>' open the Compilation page.
    var open: (() -> Void)? = nil

    private func age(_ since: UInt64) -> String {
        let now = model.daemonNow()
        return Format.duration(ms: now > since ? now - since : 0)
    }

    @ViewBuilder private func who(_ label: String, task: UInt64, command: String = "") -> some View {
        if task > 0 {
            TaskLink(title: "#\(task)") { model.go(.task(task)) }
        } else if !command.isEmpty {
            Button(label) { model.openCommand(label: label, command: command) }
                .buttonStyle(.plain).linkStyle()
                .lineLimit(1).hint("Show the whole command")
                .accessibilityIdentifier("compile:command")
        } else {
            Text(label).lineLimit(1).truncationMode(.middle).hint(label)
        }
    }

    var body: some View {
        section("Compilation (all projects)", theme: theme, open: open) {
            HStack(spacing: theme.spacing) {
                Text("Compiling: \(c.used) of at most")
                Button { model.setCompileLimit(c.limit > 1 ? c.limit - 1 : 1) } label: { Image(systemName: "minus") }.touchTarget()
                    .disabled(c.limit <= 1)
                    .accessibilityIdentifier("compile:decrease")
                Text("\(c.limit)").font(theme.body(.semibold))
                Button { model.setCompileLimit(c.limit + 1) } label: { Image(systemName: "plus") }.touchTarget()
                    .accessibilityIdentifier("compile:increase")
                Text(c.cpuLine).foregroundStyle(theme.muted)
                if let p = c.policyLine { Text(p).foregroundStyle(theme.muted) }
            }
            .font(theme.body())
            .buttonStyle(.bordered)
            .hint("Lowering the limit never stops a compile that is running; it holds back new ones until the count falls.")
            .accessibilityIdentifier("compile")

            HStack(spacing: theme.spacing) {
                Text("Start another compile below")
                Button { model.setCompileCpuGate(Int(c.cpuMaxPct) - 5) } label: { Image(systemName: "minus") }.touchTarget()
                    .disabled(Int(c.cpuMaxPct) <= MonitorModel.cpuGateRange.lowerBound)
                    .accessibilityIdentifier("compile:cpu:decrease")
                Text("\(c.cpuMaxPct)% CPU").font(theme.body(.semibold))
                Button { model.setCompileCpuGate(Int(c.cpuMaxPct) + 5) } label: { Image(systemName: "plus") }.touchTarget()
                    .disabled(Int(c.cpuMaxPct) >= MonitorModel.cpuGateRange.upperBound)
                    .accessibilityIdentifier("compile:cpu:increase")
            }
            .font(theme.body())
            .buttonStyle(.bordered)
            .hint("A compile starts only while CPU use is below this. Kept across restarts; resources.toml has the starting value.")
            .accessibilityIdentifier("compile:cpu")

            HStack(spacing: theme.spacing) {
                Text("and wait")
                Button { model.setCompileSettle(Int(c.settleS) - 5) } label: { Image(systemName: "minus") }.touchTarget()
                    .disabled(c.settleS == 0)
                    .accessibilityIdentifier("compile:settle:decrease")
                Text("\(c.settleS) s").font(theme.body(.semibold))
                Button { model.setCompileSettle(Int(c.settleS) + 5) } label: { Image(systemName: "plus") }.touchTarget()
                    .disabled(Int(c.settleS) >= MonitorModel.settleMax)
                    .accessibilityIdentifier("compile:settle:increase")
                Text("after the last one started")
            }
            .font(theme.body())
            .buttonStyle(.bordered)
            .accessibilityIdentifier("compile:settle")

            if let refused = model.lastResult {
                Text(refused).font(theme.body()).foregroundStyle(theme.attention)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("compile:refused")
            }

            if c.running.isEmpty {
                Text("Nothing is compiling.").font(theme.body()).foregroundStyle(theme.muted)
            }
            ForEach(Array(c.running.enumerated()), id: \.offset) { i, r in
                HStack(spacing: theme.spacing) {
                    who(r.label, task: r.task, command: r.command)
                    let detail = Format.compileHolder(project: r.project, title: r.title)
                    if r.task > 0 && !detail.isEmpty {
                        Text(detail).hint(r.title.isEmpty ? r.project : "\(r.project) - \(r.title)")
                    }
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
                    who(w.label, task: w.task, command: w.command)
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
        .sheet(item: Binding(get: { model.commandDialog.map(CommandSheetItem.init) }, set: { if $0 == nil { model.closeCommand() } })) { item in
            CommandSheet(dialog: item.dialog) { model.closeCommand() }
        }
    }
}

private struct CommandSheetItem: Identifiable {
    let dialog: CommandDialog
    var id: String { dialog.title + "\u{0}" + dialog.command }
    init(_ d: CommandDialog) { dialog = d }
}

/// A command shown whole: scrollable, monospaced, selectable, with Copy and Close.
struct CommandSheet: View {
    @Environment(\.theme) var theme
    let dialog: CommandDialog
    let close: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text(dialog.title).font(theme.body(.semibold)).lineLimit(2)
            ScrollView([.vertical, .horizontal]) {
                Text(dialog.command).font(.system(.body, design: .monospaced)).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .accessibilityIdentifier("command:text")
            }
            .frame(minWidth: 480, minHeight: 200)
            HStack {
                Spacer()
                Button("Copy") { copyToPasteboard(dialog.command) }.accessibilityIdentifier("command:copy")
                Button("Close", action: close).keyboardShortcut(.cancelAction).accessibilityIdentifier("command:close")
            }
        }
        .padding()
        .frame(minWidth: 520, minHeight: 300)
    }
}

/// A task's page: step progress, activity, the live log, the plan, checks,
/// the decision card and the actions; copy buttons; why exploring was skipped.
struct TaskDetailPage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let id: UInt64
    /// A phone: the picture sits above the text and a step's parts wrap, so nothing is wider than the screen.
    var compact = false

    var body: some View {
        RestoringScroll(model: model, ready: model.taskDetail?.hasRow == true && model.taskDetail?.row.id == id) {
            if let d = model.taskDetail, d.hasRow, d.row.id == id {
                content(d)
            } else if model.connected, let r = model.sidebarRow(id) {
                // The sidebar already has this task: draw it now, fill in the rest when masond sends the page.
                content(Self.placeholder(r), loading: true)
            } else {
                Text(model.connected ? "Loading task #\(id)…" : "Not connected")
                    .font(theme.body()).foregroundStyle(theme.muted).padding(theme.spacingLarge)
            }
        }
    }

    static func placeholder(_ r: TaskRow) -> TaskDetailView {
        var d = TaskDetailView()
        d.row = r
        return d
    }

    @ViewBuilder func content(_ d: TaskDetailView, loading: Bool = false) -> some View {
        let r = d.row
        VStack(alignment: .leading, spacing: theme.spacingLarge) {
            let header = compact
                ? AnyLayout(VStackLayout(alignment: .leading, spacing: theme.spacing))
                : AnyLayout(HStackLayout(alignment: .top, spacing: theme.spacingLarge))
            header {
            if let art = Mascot.illustration(r.state) {
                let side = compact ? theme.illustration / 2 : theme.illustration
                art.resizable().scaledToFit()
                    .frame(width: side, height: side)
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
                Text(Format.cpuTaskLine(permille: d.cpuPermille)).font(theme.body()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("cpu")
                let copies = compact
                    ? AnyLayout(VStackLayout(alignment: .leading, spacing: theme.spacingSmall))
                    : AnyLayout(HStackLayout(spacing: theme.spacingLarge))
                copies {
                    CopyButton(label: r.key, value: r.key)
                    ForEach(CopyTarget.detail(branch: d.branch, worktree: d.worktree), id: \.label) { t in
                        CopyValueButton(target: t) { (model.copier ?? copyToPasteboard)($0) }
                    }
                }
            }
            }

            let progressRow = compact
                ? AnyLayout(VStackLayout(alignment: .leading, spacing: theme.spacingSmall))
                : AnyLayout(HStackLayout(spacing: theme.spacing))
            progressRow {
                LiveProgress(live: model.progress.detail, fallback: r.progress) {
                    ProgressCells(clock: model.live, cells: $0, slow: r.slow)
                }
                if !r.typical.isEmpty { Text(r.typical).font(theme.small()).foregroundStyle(theme.muted) }
            }
            LiveProgress(live: model.progress.detail, fallback: r.progress) { cells in
                if let c = cells.running { ProgressDetail(model: model, cell: c) }
            }

            ActionButtons(model: model, row: r, hasApprovalView: d.hasApproval)
            if loading {
                Text("Loading details…").font(theme.body()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("detail:loading")
            }

            if !r.blockers.isEmpty {
                HStack(spacing: theme.spacingSmall) {
                    Text("waiting for").font(theme.body()).foregroundStyle(theme.muted)
                    ForEach(r.blockers, id: \.self) { b in
                        TaskLink(title: "#\(b)", font: theme.body()) { model.go(.task(b)) }
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
                        TaskLink(title: "#\(b)", font: theme.body()) { model.go(.task(b)) }
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
                    let stepRow = compact
                        ? AnyLayout(VStackLayout(alignment: .leading, spacing: 0))
                        : AnyLayout(HStackLayout(spacing: theme.spacing))
                    stepRow {
                        Text("run \(s.run)").foregroundStyle(theme.muted)
                        Text(s.name.isEmpty ? s.category : s.name).font(theme.body(.semibold))
                        Text("attempt \(s.attempt)").foregroundStyle(theme.muted)
                        Text(s.outcome)
                        LiveProgress(live: model.progress.detail, fallback: r.progress) { cells in
                            if let c = cells.first(where: { $0.step == s.name && $0.state == .part && $0.run == s.run }) {
                                Text(c.countText).foregroundStyle(theme.muted)
                                Text(c.timeText(now: model.daemonNow())).foregroundStyle(c.isStale(now: model.daemonNow()) ? theme.slow : theme.muted)
                            }
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
            LiveLogSection(model: model, live: model.live)
        }
        .padding(theme.spacingLarge)
    }
}

struct LandingPage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel

    var body: some View {
        RestoringScroll(model: model) {
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
                            TaskLink(title: "\(r.key) \(r.title)") { model.go(.task(r.id)) }
                        }
                    }
                    if !l.items.isEmpty {
                        section("Landing tasks", theme: theme) { LandingItems(model: model, items: l.items) }
                    }
                    section("Batches", theme: theme) {
                        if l.batches.isEmpty { Text("No batch has formed yet.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(l.batches, id: \.id) { b in
                            Text(batchText(b)).font(theme.body())
                            if b.hasCi {
                                HStack(spacing: theme.spacing) {
                                    Text(batchCiText(b.ci)).font(theme.body())
                                        .foregroundStyle(batchCiFailed(b.ci) ? theme.attention : theme.muted)
                                    if let url = batchCiURL(b.ci) { WebLink(title: "CI run", url: url) }
                                }
                            }
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

/// The CI line under a batch: per workflow "pr-gate: 11 jobs · 1 queued · 2 running · 7 passed",
/// "· N failed" when any failed, and "queued for a runner" while no job has started.
func batchCiText(_ ci: Mason_V1_BatchCi) -> String {
    let parts = ci.workflows.map { w -> String in
        let total = w.queued + w.running + w.passed + w.failed
        if w.running + w.passed + w.failed == 0 { return "\(w.workflow): queued for a runner" }
        var t = "\(w.workflow): \(total) jobs · \(w.queued) queued · \(w.running) running · \(w.passed) passed"
        if w.failed > 0 { t += " · \(w.failed) failed" }
        return t
    }
    return "CI: " + (parts.isEmpty ? "queued for a runner" : parts.joined(separator: "; "))
}

func batchCiFailed(_ ci: Mason_V1_BatchCi) -> Bool { ci.workflows.contains { $0.failed > 0 } }

/// The run's address, only if it is http or https.
func batchCiURL(_ ci: Mason_V1_BatchCi) -> URL? {
    guard let u = URL(string: ci.url), ["http", "https"].contains(u.scheme?.lowercased() ?? "") else { return nil }
    return u
}

struct UsagePage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    var compact = false

    var body: some View {
        RestoringScroll(model: model) {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                Text("LLM usage").font(theme.title())
                ForEach(model.usage?.providers ?? [], id: \.name) { p in
                    UsageAccount(p: p)
                }
                if let n = model.usage?.note { Text(n).font(theme.body()).foregroundStyle(theme.muted) }
                if compact { PhoneSessionStepper(model: model) } else { SessionStepper(model: model) }
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
        RestoringScroll(model: model) {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                Text("Health").font(theme.title())
                HealthConnectionLine(model: model)
                if let h = model.health {
                    section("Resources", theme: theme) {
                        ForEach(h.resources, id: \.name) { r in
                            Text("\(r.name) (\(r.kind)): \(r.used) of \(r.limit) in use").font(theme.body())
                        }
                    }
                    Text(h.cpuLine).font(theme.body())
                    if let p = h.policyLine { Text(p).font(theme.body()) }
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

/// The task page's log: observes only the live state, so a log line redraws this and nothing else.
struct LiveLogSection: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    @ObservedObject var live: LiveState

    var body: some View {
        if !live.liveLog.isEmpty {
            section("Log", theme: theme) {
                LinkedText(model: model, text: live.liveLog.joined(separator: "\n"), font: theme.mono(), selectable: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(theme.spacing)
                    .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.surface))
            }
        }
    }
}

/// The Health page's connection line, which shows the sequence number.
struct HealthConnectionLine: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    @ObservedObject var live: LiveState

    init(model: MonitorModel) {
        self.model = model
        self.live = model.live
    }

    var body: some View {
        Text(model.connected ? "Connected to masond. Event log at #\(live.lastSeq)." : "Not connected.")
            .font(theme.body())
    }
}

/// "Landing now" (in a batch) above "Queued", one line per task.
struct LandingItems: View {
    @ObservedObject var model: MonitorModel
    let items: [Mason_V1_TaskLanding]
    @Environment(\.theme) private var theme

    var body: some View {
        let g = landingGroups(items)
        VStack(alignment: .leading, spacing: theme.spacing) {
            if !g.landing.isEmpty {
                Text("Landing now").font(theme.body()).foregroundStyle(theme.muted)
                ForEach(g.landing, id: \.task) { i in
                    TaskLink(title: landingItemText(i), font: theme.body()) { model.go(.task(i.task)) }
                }
            }
            if !g.queued.isEmpty {
                Text("Queued").font(theme.body()).foregroundStyle(theme.muted)
                ForEach(g.queued, id: \.task) { i in
                    TaskLink(title: landingItemText(i), font: theme.body()) { model.go(.task(i.task)) }
                }
            }
        }
    }
}

/// Tasks in a batch, then tasks still queued (in queue order).
func landingGroups(_ items: [Mason_V1_TaskLanding]) -> (landing: [Mason_V1_TaskLanding], queued: [Mason_V1_TaskLanding]) {
    (items.filter { $0.status == "in_batch" },
     items.filter { $0.status != "in_batch" }.sorted { $0.position < $1.position })
}

/// "▰▰▱▱" for phase 2 of 4.
func phaseBar(_ index: UInt32, _ total: UInt32) -> String {
    let n = Int(min(index, total))
    return String(repeating: "▰", count: n) + String(repeating: "▱", count: Int(total) - n)
}

/// "#5 Title: batch #3, running suites, phase 2 of 4 ▰▰▱▱, CI green, 4 min" or
/// "#6 Title: queued #1, behind 1 batch".
func landingItemText(_ i: Mason_V1_TaskLanding) -> String {
    if i.status == "in_batch" {
        var t = "\(i.key) \(i.title): batch #\(i.batch), \(i.phase), phase \(i.phaseIndex) of \(i.phaseTotal) \(phaseBar(i.phaseIndex, i.phaseTotal))"
        if !i.ciState.isEmpty { t += ", CI \(i.ciState)" }
        return t + ", \(i.phaseMs / 60000) min"
    }
    return "\(i.key) \(i.title): queued #\(i.position), \(i.why)"
}

/// A detail page for one section of home (`mason://section/<name>`): only that section, with
/// the room to show a task's progress, step, why it waits, time in state, CPU share
/// and model on one wide row. The rows are the sidebar's, so they come in its order.
struct SectionPage: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let name: String
    /// A phone: each row is a column, not one wide line.
    var compact = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: theme.spacingLarge) {
                switch name {
                case "active_models":
                    section(MonitorModel.sectionTitle(name), theme: theme) {
                        let active = model.home?.active ?? []
                        if active.isEmpty { Text("Nothing is running.").font(theme.body()).foregroundStyle(theme.muted) }
                        ForEach(active, id: \.task) { a in
                            HStack(spacing: theme.spacing) {
                                Text(a.display.isEmpty ? a.model : a.display).font(theme.body(.semibold))
                                TaskLink(title: "\(a.project) #\(a.task)") { model.go(.task(a.task)) }
                                Text(a.step).foregroundStyle(theme.muted)
                            }
                            .font(theme.body())
                            .accessibilityIdentifier("active:\(a.task)")
                        }
                        SessionStepper(model: model)
                    }
                case "cpu":
                    CpuSection(model: model, cpu: model.cpu)
                case "compilation":
                    if let h = model.home, h.hasCompilation {
                        CompilationSection(model: model, c: h.compilation)
                    } else {
                        section(MonitorModel.sectionTitle(name), theme: theme) {
                            Text("Nothing is compiling.").font(theme.body()).foregroundStyle(theme.muted)
                        }
                    }
                default:
                    StageRows(model: model, name: name, compact: compact)
                }
            }
            .padding(theme.spacingLarge)
        }
        .accessibilityIdentifier("section-page:\(name)")
    }
}

/// The wide task rows of a stage page. Testing adds suite progress and Skip testing.
struct StageRows: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let name: String
    var compact = false

    var body: some View {
        let rows = model.sectionRows(name)
        section(MonitorModel.sectionTitle(name), theme: theme, state: name) {
            if name == "testing", model.canAct, model.skippableRows(rows).count > 0 {
                Button("Skip testing for all") { model.skipAllTests(rows) }
                    .buttonStyle(.bordered).accessibilityIdentifier("testing:skip-all")
            }
            if rows.isEmpty { Text("Nothing here.").font(theme.body()).foregroundStyle(theme.muted) }
            ForEach(rows, id: \.id) { r in row(r) }
        }
    }

    @ViewBuilder func row(_ r: TaskRow) -> some View {
        let layout = compact
            ? AnyLayout(VStackLayout(alignment: .leading, spacing: theme.spacingSmall))
            : AnyLayout(HStackLayout(alignment: .firstTextBaseline, spacing: theme.spacing))
        layout {
            LiveProgress(live: model.progress.entry(r.id), fallback: r.progress) {
                ProgressCells(clock: model.live, cells: $0, slow: r.slow)
            }
            Button { model.go(.task(r.id)) } label: {
                Text("\(r.project) \(r.key)  \(r.title)").font(theme.body()).lineLimit(3)
                    .multilineTextAlignment(.leading)
            }.buttonStyle(.plain)
            if !compact { Spacer(minLength: 0) }
            detail(r)
            if name == "testing", model.canAct, r.allowedActions.contains("skip_test") {
                Button("Skip testing") { model.skipTest(r.id, step: r.step) }
                    .buttonStyle(.bordered).accessibilityIdentifier("testing:skip:\(r.id)")
            }
        }
        .accessibilityIdentifier("stage-row:\(name):\(r.id)")
    }

    @ViewBuilder func detail(_ r: TaskRow) -> some View {
        let parts = [r.step.isEmpty ? "" : r.step,
                     MonitorModel.suiteProgress(r),
                     r.whyWaiting,
                     Format.duration(ms: r.inStateMs),
                     r.display.isEmpty ? r.model : r.display]
            .filter { !$0.isEmpty }
        Text(parts.joined(separator: " · ")).font(theme.small())
            .foregroundStyle(r.slow ? theme.slow : theme.muted).lineLimit(2)
    }
}
