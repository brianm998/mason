// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI

/// Progress cells (Part 17): filled means finished; hatched means waiting, labelled with what
/// for; empty means not started. A running cell with a real total (reported or counted) fills
/// in proportion to it; with only a typical total from past runs it fills the same way, lighter;
/// with neither it stays an outline. A percentage is never estimated. A step running past its
/// typical time, or quiet past its usual gap, turns amber, never red: slow is not broken.
struct ProgressCells: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let cells: [Mason_V1_ProgressCell]
    var slow: Bool = false

    var body: some View {
        HStack(spacing: theme.spacingSmall) {
            ForEach(Array(cells.enumerated()), id: \.offset) { _, c in
                let quiet = c.state == .part && c.isStale(now: model.daemonNow())
                Cell(state: c.state, fill: c.state == .part ? c.fill : .outline,
                     tint: (slow || quiet) && c.state == .part ? theme.slow : theme.accent)
                    .frame(width: theme.cell, height: theme.cell)
                    .help(helpText(c))
                    .accessibilityLabel(c.spokenLabel)
            }
        }
    }

    /// The step name and its label; a running step with a count adds it (no clock text).
    private func helpText(_ c: Mason_V1_ProgressCell) -> String {
        let count = c.state == .part ? c.countText : ""
        let label = c.label.isEmpty ? "" : ": \(c.label)"
        return count.isEmpty ? "\(c.step)\(label)" : "\(c.step)\(label), \(count)"
    }
}

extension Array where Element == Mason_V1_ProgressCell {
    /// The step that is running now and has a live record.
    var running: Mason_V1_ProgressCell? { first { $0.state == .part && $0.run != 0 } }
}

/// One line for the running step: its count by the rule for numbers, its message cut to fit, and
/// how long ago it last moved. The time counts up on this device's clock between pushes (corrected
/// by the daemon's clock) and turns amber, with "no output for", past the step's usual gap. At a
/// narrow width the message goes first. Only the count and message are read aloud: the ticking
/// time is not announced every second.
struct ProgressLine: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let cell: Mason_V1_ProgressCell

    var body: some View {
        TimelineView(.periodic(from: .now, by: 1)) { ctx in
            let now = model.daemonNow(ctx.date)
            let stale = cell.isStale(now: now)
            ViewThatFits(in: .horizontal) {
                parts(now: now, stale: stale, message: true)
                parts(now: now, stale: stale, message: false)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel([cell.countText, cell.tokensCompact, cell.message, stale ? "no output for a while" : ""]
                .filter { !$0.isEmpty }.joined(separator: ", "))
            .accessibilityIdentifier("progress-line:\(cell.step)")
        }
    }

    private func parts(now: UInt64, stale: Bool, message: Bool) -> some View {
        HStack(spacing: theme.spacingSmall) {
            let count = cell.countText
            if !count.isEmpty { Text(count).foregroundStyle(theme.text).lineLimit(1).fixedSize() }
            let tokens = cell.tokensCompact
            if !tokens.isEmpty {
                Text(tokens).foregroundStyle(theme.muted).lineLimit(1).fixedSize()
                    .accessibilityIdentifier("progress-tokens:\(cell.step)")
            }
            if message && !cell.message.isEmpty {
                Text(cell.message).foregroundStyle(theme.muted).lineLimit(1)
            }
            let time = cell.timeText(now: now)
            if !time.isEmpty {
                Text(time).foregroundStyle(stale ? theme.slow : theme.muted).lineLimit(1).fixedSize()
            }
        }
        .font(theme.small())
    }
}

/// The running step's whole record on the task page: count and time, sub-counts, message, the
/// agent's todo list with each item's status, the output figures, and where each number came from.
struct ProgressDetail: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let cell: Mason_V1_ProgressCell

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            ProgressLine(model: model, cell: cell)
            if !cell.message.isEmpty { Text(cell.message).font(theme.body()) }
            ForEach(Array(cell.subs.enumerated()), id: \.offset) { _, s in
                let text = Format.countText(
                    unit: s.unit, done: UInt64(s.done), doneSource: s.doneSource,
                    total: UInt64(s.total), totalSource: s.totalSource, totalLow: 0)
                Text("\(s.name): \(text)").font(theme.body())
            }
            ForEach(Array(cell.todo.enumerated()), id: \.offset) { _, t in
                HStack(spacing: theme.spacingSmall) {
                    Text(t.status == "done" ? "✓" : t.status == "doing" ? "▶" : "○")
                        .accessibilityHidden(true)
                    Text(t.text).strikethrough(t.status == "done")
                        .foregroundStyle(t.status == "pending" ? theme.muted : theme.text)
                    Text(t.status).font(theme.small()).foregroundStyle(theme.muted)
                }
                .font(theme.body())
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(t.status): \(t.text)")
            }
            if cell.lines > 0 || cell.bytes > 0 {
                Text("output \(Format.group(cell.lines)) lines, \(Format.group(cell.bytes)) bytes")
                    .font(theme.small()).foregroundStyle(theme.muted)
            }
            if cell.hasTokens {
                let figures = Format.tokensDetail(cell.tokens)
                if !figures.isEmpty {
                    Text("tokens " + figures.joined(separator: ", "))
                        .font(theme.small()).foregroundStyle(theme.muted)
                        .accessibilityIdentifier("progress-token-detail:\(cell.step)")
                }
            }
            let sources = [
                cell.doneSource == .sourceNone ? "" : "count \(Format.sourceWords(cell.doneSource, samples: cell.historySamples))",
                cell.totalSource == .sourceNone ? "" : "total \(Format.sourceWords(cell.totalSource, samples: cell.historySamples))",
            ].filter { !$0.isEmpty }
            if !sources.isEmpty {
                Text(sources.joined(separator: "; ")).font(theme.small()).foregroundStyle(theme.muted)
            }
        }
        .accessibilityIdentifier("progress:\(cell.step)")
    }
}

struct Cell: View {
    @Environment(\.theme) var theme
    let state: Mason_V1_CellState
    var fill: CellFill = .outline
    let tint: Color

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 2)
        ZStack {
            switch state {
            case .filled:
                shape.fill(tint)
            case .part:
                shape.stroke(tint, lineWidth: 1)
                switch fill {
                case .proportional(let f):
                    GeometryReader { g in
                        Rectangle().fill(tint).frame(width: g.size.width * f).clipShape(shape)
                    }
                case .typical(let f):
                    // Lighter than a real count: this is the typical figure, not a measured one.
                    GeometryReader { g in
                        Rectangle().fill(tint.opacity(0.4)).frame(width: g.size.width * f).clipShape(shape)
                    }
                case .outline:
                    EmptyView()
                }
            case .hatched:
                shape.stroke(tint, lineWidth: 1)
                Hatch().stroke(tint, lineWidth: 1).clipShape(shape)
            default:
                shape.stroke(theme.border, lineWidth: 1)
            }
        }
    }
}

struct Hatch: Shape {
    func path(in r: CGRect) -> Path {
        var p = Path()
        var x = -r.height
        while x < r.width {
            p.move(to: CGPoint(x: x, y: r.height))
            p.addLine(to: CGPoint(x: x + r.height, y: 0))
            x += 4
        }
        return p
    }
}

/// A count tile: a large count over a one-line label, sized to its label so a
/// narrow window squeezes other text and never a tile or label; tinted when
/// the state wants the owner; a dash while disconnected.
struct CountTile: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let count: StateCount

    var body: some View {
        let wants = count.wantsYou && model.connected
        VStack(spacing: 0) {
            Text(model.tileCount(count)).font(theme.count()).foregroundStyle(wants ? theme.attention : theme.text)
            Text(label).font(theme.small()).foregroundStyle(theme.muted).lineLimit(1).fixedSize()
        }
        .padding(.horizontal, theme.spacing)
        .frame(minWidth: theme.tileMinWidth, minHeight: theme.tileHeight, maxHeight: theme.tileHeight)
        .fixedSize(horizontal: true, vertical: false)
        .background(RoundedRectangle(cornerRadius: theme.radius).fill(wants ? theme.attentionTint : theme.surface))
        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(wants ? theme.attention : theme.border, lineWidth: 1))
        .contentShape(Rectangle())
        .onTapGesture { model.tileClicked(state: count.state, clickCount: NSApp.currentEvent?.clickCount ?? 1) }
        .help(model.tileTooltip(count))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(model.tileCount(count)) \(label)")
        .accessibilityIdentifier("tile:\(count.state)")
        .accessibilityAddTraits(.isButton)
    }

    var label: String { model.tileLabel(count) }
}

struct Pill: ViewModifier {
    @Environment(\.theme) var theme
    var prominent = false
    func body(content: Content) -> some View {
        content
            .font(theme.body(prominent ? .semibold : .regular))
            .padding(.horizontal, theme.spacing)
            .padding(.vertical, theme.spacingSmall)
            .background(RoundedRectangle(cornerRadius: theme.radius).fill(prominent ? theme.accent : theme.surface))
            .foregroundStyle(prominent ? Color.white : theme.text)
            .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.border, lineWidth: prominent ? 0 : 1))
    }
}

extension View {
    func pill(prominent: Bool = false) -> some View { modifier(Pill(prominent: prominent)) }
}

func copyToPasteboard(_ s: String) {
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(s, forType: .string)
}

/// A small button that copies something and says what.
struct CopyButton: View {
    @Environment(\.theme) var theme
    let label: String
    let value: String
    var body: some View {
        Button { copyToPasteboard(value) } label: {
            HStack(spacing: theme.spacingSmall) {
                Image(systemName: "doc.on.doc")
                Text(label)
            }
            .font(theme.small())
        }
        .buttonStyle(.plain)
        .foregroundStyle(theme.muted)
        .help("Copy \(value)")
    }
}

/// The decision card (Part 10): the ask, why only the owner can settle it, 2 to
/// 4 options with one recommended, "Other" with a text box, and the evidence.
/// An answer carries the card's hash, so a late click on a stale card is refused.
struct DecisionCardView: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let task: UInt64
    let card: Mason_V1_DecisionCard
    @State private var other = ""
    @State private var selection = OptionSelection(count: 0)
    @FocusState private var cardFocused: Bool
    @FocusState private var otherFocused: Bool

    private func sendOther() {
        guard let text = otherAnswer(other) else { return }
        model.answer(task: task, text, cardHash: card.hash)
        other = ""
    }

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text(card.ask).font(theme.title()).foregroundStyle(theme.text).fixedSize(horizontal: false, vertical: true)
            if !card.why.isEmpty {
                Text(card.why).font(theme.body()).foregroundStyle(theme.muted).fixedSize(horizontal: false, vertical: true)
            }
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                ForEach(Array(card.options.enumerated()), id: \.offset) { i, o in
                    if model.canAct {
                        Button { model.answer(task: task, String(i + 1), cardHash: card.hash) } label: {
                            optionLabel(i, o)
                                .pill(prominent: o.recommended)
                                .overlay(RoundedRectangle(cornerRadius: theme.radius)
                                    .stroke(theme.text, lineWidth: selection.index == i ? 2 : 0))
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("card:\(task):option:\(i + 1)")
                    } else {
                        optionLabel(i, o).font(theme.body()).foregroundStyle(theme.text)
                    }
                }
                if model.canAct {
                    HStack(alignment: .top) {
                        ZStack(alignment: .topLeading) {
                            // Multi-line: Enter sends, Shift-Enter starts a new line.
                            TextEditor(text: $other)
                                .font(theme.body())
                                .scrollContentBackground(.hidden)
                                .focused($otherFocused)
                                .frame(minHeight: theme.cell * 2.2, maxHeight: theme.cell * 8)
                                .fixedSize(horizontal: false, vertical: true)
                                .onKeyPress(.return, phases: .down) { press in
                                    switch otherBoxKey(shift: press.modifiers.contains(.shift)) {
                                    case .send: sendOther(); return .handled
                                    case .newline: return .ignored
                                    }
                                }
                                .accessibilityIdentifier("card:\(task):other")
                            if other.isEmpty {
                                Text("Other… (Enter sends, Shift-Enter for a new line)")
                                    .font(theme.body()).foregroundStyle(theme.muted)
                                    .padding(.leading, 5).padding(.top, 0).allowsHitTesting(false)
                            }
                        }
                        .padding(4)
                        .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.surface))
                        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.muted.opacity(0.5), lineWidth: 1))
                        Button("Send") { sendOther() }
                            .disabled(otherAnswer(other) == nil)
                            .accessibilityIdentifier("card:\(task):send")
                    }
                }
            }
            if !card.evidence.isEmpty {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(card.evidence.enumerated()), id: \.offset) { _, l in
                        Text("| \(l)").font(theme.mono()).foregroundStyle(theme.muted)
                    }
                }
            }
            if card.origin != "agent" {
                Text("This card was \(card.origin) because the agent's did not fit.")
                    .font(theme.small()).foregroundStyle(theme.muted)
            }
            if !model.canAct && model.connected {
                Text("Only the owner can answer. You are connected as \(model.identity).")
                    .font(theme.small()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("card:\(task):not-owner")
            }
            if let refused = model.lastResult {
                Text(refused).font(theme.body()).foregroundStyle(theme.attention)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("card:\(task):refused")
            }
        }
        .padding(theme.spacingLarge)
        .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.attentionTint))
        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.attention, lineWidth: 1))
        // Arrow keys move the selection and Return answers with it, as in Claude Code's prompts.
        .focusable()
        .focused($cardFocused)
        .onKeyPress(.upArrow) { guard model.canAct else { return .ignored }; selection.up(); return .handled }
        .onKeyPress(.downArrow) { guard model.canAct else { return .ignored }; selection.down(); return .handled }
        .onKeyPress(.return) {
            guard model.canAct, let a = selection.answer else { return .ignored }
            model.answer(task: task, a, cardHash: card.hash)
            return .handled
        }
        .onAppear {
            selection = OptionSelection(count: card.options.count, recommended: card.options.firstIndex { $0.recommended })
        }
        .onChange(of: card.hash) { _, _ in
            selection = OptionSelection(count: card.options.count, recommended: card.options.firstIndex { $0.recommended })
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("card:\(task)")
    }

    private func optionLabel(_ i: Int, _ o: Mason_V1_CardOption) -> some View {
        HStack {
            Text("\(i + 1).")
            Text(o.label)
            if o.recommended {
                Text("recommended").font(theme.small()).foregroundStyle(Color.white.opacity(0.85))
            }
            Spacer(minLength: 0)
        }
    }
}

/// What a task waiting for the owner's approval waits on, in full, with Approve and Reject.
struct ApprovalView: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let task: UInt64
    let approval: Mason_V1_Approval
    @State private var reason = ""
    @State private var rejecting = false

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text(approval.waitingFor).font(theme.title()).foregroundStyle(theme.text)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(Array(approval.lines.enumerated()), id: \.offset) { _, l in
                Text("• \(l)").font(theme.body()).foregroundStyle(theme.text)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if !approval.diffHash.isEmpty {
                Text("Diff \(approval.diffHash)").font(theme.mono()).foregroundStyle(theme.muted)
                    .textSelection(.enabled)
            }
            if !approval.branch.isEmpty {
                Text("Branch \(approval.branch)").font(theme.mono()).foregroundStyle(theme.muted)
                    .textSelection(.enabled)
            }
            if !approval.diffCommand.isEmpty {
                HStack {
                    Text(approval.diffCommand).font(theme.mono()).foregroundStyle(theme.muted)
                        .textSelection(.enabled)
                    Button("Copy") { copyToPasteboard(approval.diffCommand) }
                        .accessibilityIdentifier("approval:\(task):copy-diff")
                }
            }
            if model.canAct {
                HStack(spacing: theme.spacing) {
                    Button { model.approve(task, approval) } label: { Text("Approve").pill(prominent: true) }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("approval:\(task):approve")
                    Button { rejecting.toggle() } label: { Text("Reject").pill(prominent: false) }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("approval:\(task):reject")
                }
                if rejecting {
                    HStack(alignment: .top) {
                        TextField("Why? (optional)", text: $reason, axis: .vertical)
                            .textFieldStyle(.roundedBorder).font(theme.body())
                            .onSubmit { model.reject(task, reason: reason); reason = ""; rejecting = false }
                            .accessibilityIdentifier("approval:\(task):reason")
                        Button("Send rejection") { model.reject(task, reason: reason); reason = ""; rejecting = false }
                            .accessibilityIdentifier("approval:\(task):send-reject")
                    }
                }
            } else if model.connected {
                Text("Only the owner can approve or reject. You are connected as \(model.identity).")
                    .font(theme.small()).foregroundStyle(theme.muted)
            }
            if let refused = model.lastResult {
                Text(refused).font(theme.body()).foregroundStyle(theme.attention)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("approval:\(task):refused")
            }
        }
        .padding(theme.spacingLarge)
        .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.attentionTint))
        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.attention, lineWidth: 1))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("approval:\(task)")
    }
}

/// The actions a task offers now: exactly the daemon's `allowed_actions`.
struct ActionButtons: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let row: TaskRow
    var hasApprovalView = false

    var body: some View {
        HStack(spacing: theme.spacing) {
            // Approve and Reject have their own view (ApprovalView) wherever there is one; a
            // non-owner sees no buttons at all for what only the owner may do.
            ForEach(row.allowedActions.filter { shown($0) }, id: \.self) { a in
                Button { run(a) } label: { Text(label(a)).pill(prominent: a == "approve") }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("action:\(row.id):\(a)")
            }
        }
    }

    func shown(_ a: String) -> Bool {
        if a == "answer" || a == "edit" { return false }
        if hasApprovalView && (a == "approve" || a == "reject") { return false }
        if !model.canAct && ["approve", "reject"].contains(a) { return false }
        return true
    }

    func label(_ a: String) -> String {
        switch a {
        case "skip_test": return "Skip test"
        case "land_now": return "Land now"
        case "explore_anyway": return "Explore anyway"
        case "open_session": return "Open session"
        default: return a.prefix(1).uppercased() + a.dropFirst()
        }
    }

    func run(_ a: String) {
        switch a {
        case "cancel": model.cancel(row.id)
        case "retry": model.retry(row.id)
        case "approve": model.approve(row.id)
        case "reject": model.reject(row.id)
        case "explore_anyway": model.exploreAnyway(row.id)
        case "land_now": model.landNow(row.id)
        case "force": model.force(row.id)
        case "skip_test": model.skipTest(row.id, step: row.step)
        case "open_session": copyToPasteboard("mason task open \(row.id)")
        default: break
        }
    }
}

/// A proposal or an improvement: who noticed it, how bad, where it goes, the proof, and (for an
/// improvement without write access) what would go upstream. Edit is offered only while it waits.
struct ProposalView: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let row: TaskRow
    let info: Mason_V1_ProposalInfo
    let title: String
    @State private var editing = false

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text("\(info.source) · \(info.severity) · \(info.route == "pipeline" ? "MASON's own repository" : "this project")")
                .font(theme.body(.semibold)).foregroundStyle(theme.text)
                .accessibilityIdentifier("proposal:\(row.id):tag")
            if info.filedBy != 0 {
                Text("Filed by the agent of task #\(info.filedBy)").font(theme.small()).foregroundStyle(theme.muted)
            }
            if !info.evidence.isEmpty {
                Text("Evidence").font(theme.body(.semibold)).foregroundStyle(theme.text)
                Text(info.evidence).font(theme.mono()).foregroundStyle(theme.text).textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityIdentifier("proposal:\(row.id):evidence")
            }
            if info.status == "rejected" {
                Text("Rejected\(info.rejectReason.isEmpty ? "" : ": \(info.rejectReason)")").font(theme.body()).foregroundStyle(theme.muted)
            }
            if !info.branch.isEmpty {
                Text("Branch \(info.branch) (\(info.access) access)\(info.pr != 0 ? ", upstream pull request #\(info.pr) \(info.prState)" : "")")
                    .font(theme.mono()).foregroundStyle(theme.muted).textSelection(.enabled)
            }
            if !info.channel.isEmpty {
                Text("This machine follows the channel \(info.channel)").font(theme.small()).foregroundStyle(theme.muted)
                    .accessibilityIdentifier("proposal:\(row.id):channel")
            }
            if model.canAct {
                HStack(spacing: theme.spacing) {
                    if row.allowedActions.contains("edit") {
                        Button("Edit") { editing = true }.accessibilityIdentifier("proposal:\(row.id):edit")
                    }
                    if info.access == "fork" && info.pr == 0 {
                        Button("Preview what would go upstream") { model.publish(row.id, confirm: false) }
                            .accessibilityIdentifier("proposal:\(row.id):preview")
                        if info.publishState == "previewed" {
                            Button("Send it upstream") { model.publish(row.id, confirm: true) }
                                .accessibilityIdentifier("proposal:\(row.id):publish")
                        }
                    }
                }
            }
            if let p = model.publishPreview, info.access == "fork" {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Redacted version (\(p.redactions) removed), no client code attached").font(theme.small()).foregroundStyle(theme.muted)
                    Text(p.title).font(theme.body(.semibold))
                    Text(p.body).font(theme.body()).textSelection(.enabled)
                    ForEach(p.files, id: \.self) { Text("changes \($0)").font(theme.mono()).foregroundStyle(theme.muted) }
                    if let b = p.blocked { Text("Not publishable: \(b)").foregroundStyle(theme.attention) }
                }
                .accessibilityIdentifier("proposal:\(row.id):preview-text")
            }
        }
        .padding(theme.spacingLarge)
        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.border, lineWidth: 1))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("proposal:\(row.id)")
        .sheet(isPresented: $editing) {
            EditProposalSheet(model: model, task: row.id, title: title, text: info.text, evidence: info.evidence, isPresented: $editing)
        }
    }
}

/// Change a waiting proposal's title, text or evidence. Nothing else about it changes.
struct EditProposalSheet: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let task: UInt64
    @State var title: String
    @State var text: String
    @State var evidence: String
    @Binding var isPresented: Bool

    init(model: MonitorModel, task: UInt64, title: String, text: String, evidence: String, isPresented: Binding<Bool>) {
        self.model = model
        self.task = task
        _title = State(initialValue: title)
        _text = State(initialValue: text)
        _evidence = State(initialValue: evidence)
        _isPresented = isPresented
    }

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text("Edit proposal #\(task)").font(theme.title())
            TextField("Title", text: $title).textFieldStyle(.roundedBorder).accessibilityIdentifier("edit-proposal:title")
            Text("Text").font(theme.small()).foregroundStyle(theme.muted)
            TextEditor(text: $text).font(theme.body()).frame(minHeight: 100)
                .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.border))
                .accessibilityIdentifier("edit-proposal:text")
            Text("Evidence (a file and line, or the output seen)").font(theme.small()).foregroundStyle(theme.muted)
            TextEditor(text: $evidence).font(theme.mono()).frame(minHeight: 80)
                .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.border))
                .accessibilityIdentifier("edit-proposal:evidence")
            HStack {
                Spacer()
                Button("Cancel") { isPresented = false }
                Button("Save") {
                    model.editProposal(task, title: title, body: text, evidence: evidence)
                    isPresented = false
                }
                .keyboardShortcut(.defaultAction)
                .disabled(title.trimmingCharacters(in: .whitespaces).isEmpty)
                .accessibilityIdentifier("edit-proposal:save")
            }
        }
        .padding(theme.spacingLarge)
        .frame(width: 520)
    }
}

/// "Improve the pipeline": type what MASON should change about itself.
struct ImproveSheet: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    @Binding var isPresented: Bool
    @State private var text = ""

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text("Improve the pipeline").font(theme.title())
            Text("MASON files this as a task on its own repository. Without write access the change goes to your fork; nothing goes upstream until you have read the redacted version and said so.")
                .font(theme.small()).foregroundStyle(theme.muted).fixedSize(horizontal: false, vertical: true)
            TextEditor(text: $text).font(theme.body()).frame(minHeight: 120)
                .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.border))
                .accessibilityIdentifier("improve:text")
            HStack {
                Spacer()
                Button("Cancel") { isPresented = false }
                Button("Improve") {
                    model.improve(project: model.selfProject, text: text)
                    isPresented = false
                }
                .keyboardShortcut(.defaultAction)
                .disabled(text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || model.selfProject.isEmpty)
                .accessibilityIdentifier("improve:send")
            }
        }
        .padding(theme.spacingLarge)
        .frame(width: 480)
    }
}
