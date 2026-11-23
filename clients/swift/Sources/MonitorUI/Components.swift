// SPDX-License-Identifier: GPL-3.0-or-later

#if canImport(AppKit)
import AppKit
#else
import UIKit
#endif
import MasonMascot
import MasonProtocol
import MasonTokens
import MonitorCore
import SwiftUI

/// Progress cells (Part 17): filled means finished; hatched means waiting, labelled with what
/// for; empty means not started. A running cell with a real total (reported or counted) fills
/// in proportion to it; with only a typical total from past runs it fills the same way, lighter;
/// with neither it stays an outline. A percentage is never estimated. A step running past its
/// typical time, or quiet past its usual gap, turns amber, never red: slow is not broken.
struct ProgressCells: View {
    @Environment(\.theme) var theme
    let clock: LiveState
    let cells: [Mason_V1_ProgressCell]
    var slow: Bool = false

    var body: some View {
        HStack(spacing: theme.spacingSmall) {
            ForEach(Array(cells.enumerated()), id: \.offset) { _, c in
                let quiet = c.state == .part && c.isStale(now: clock.daemonNow())
                Cell(state: c.state, fill: c.state == .part ? c.fill : .outline,
                     tint: (slow || quiet) && c.state == .part ? theme.slow : theme.accent)
                    .frame(width: theme.cell, height: theme.cell)
                    .hint(helpText(c))
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

/// Reads a task's live cells from its own observed entry, falling back to the cells of the view
/// that drew it. Only this view's body runs when a progress message names the task.
struct LiveProgress<Content: View>: View {
    let live: TaskProgress
    let fallback: [Mason_V1_ProgressCell]
    @ViewBuilder let content: ([Mason_V1_ProgressCell]) -> Content

    #if DEBUG
    /// Body evaluations, for the offscreen test that counts them.
    static var evaluations: Int { get { LiveProgressCounter.n } set { LiveProgressCounter.n = newValue } }
    #endif

    var body: some View {
        #if DEBUG
        let _ = { LiveProgressCounter.n += 1 }()
        #endif
        content(live.cells ?? fallback)
    }
}

#if DEBUG
enum LiveProgressCounter { nonisolated(unsafe) static var n = 0 }
#endif

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
    let clock: LiveState
    let cell: Mason_V1_ProgressCell
    @ObservedObject private var ticker = SharedTick.shared

    var body: some View {
        Group {
            let now = clock.daemonNow(ticker.now)
            let stale = cell.isStale(now: now)
            ViewThatFits(in: .horizontal) {
                parts(now: now, stale: stale, message: true)
                parts(now: now, stale: stale, message: false)
                narrow(now: now, stale: stale)
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel([cell.countText, cell.tokensCompact, cell.message, stale ? "no output for a while" : ""]
                .filter { !$0.isEmpty }.joined(separator: ", "))
            .accessibilityIdentifier("progress-line:\(cell.step)")
        }
        .onAppear { SharedTick.shared.join() }
        .onDisappear { SharedTick.shared.leave() }
    }

    /// Last fallback, which cannot exceed the width: the count truncates, the time stays whole.
    private func narrow(now: UInt64, stale: Bool) -> some View {
        let time = cell.timeText(now: now)
        return HStack(spacing: theme.spacingSmall) {
            Text([cell.countText, cell.tokensCompact].filter { !$0.isEmpty }.joined(separator: ", "))
                .foregroundStyle(theme.text).lineLimit(1).truncationMode(.tail)
            if !time.isEmpty {
                Text(time).foregroundStyle(stale ? theme.slow : theme.muted).lineLimit(1).fixedSize()
            }
        }
        .font(theme.small())
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
    let model: MonitorModel
    let cell: Mason_V1_ProgressCell

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            ProgressLine(clock: model.live, cell: cell)
            if !cell.message.isEmpty { Text(cell.message).font(theme.body()) }
            ForEach(Array(cell.subs.enumerated()), id: \.offset) { _, s in
                let text = Format.countText(
                    unit: s.unit, done: UInt64(s.done), doneSource: s.doneSource,
                    total: UInt64(s.total), totalSource: s.totalSource, totalLow: 0)
                Text("\(s.name): \(text)").font(theme.body())
                if s.hasJobs {
                    Text(Format.jobsText(name: s.name, jobs: s.jobs)).font(theme.body())
                        .foregroundStyle(s.jobs.failed > 0 ? theme.attention : theme.text)
                }
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

/// The mascot image for a task state (the daemon's own string), drawn at the 20 pt token
/// (or the 24 pt one when `large`) times the text zoom. Decorative: hidden from accessibility,
/// never tinted. An unknown state draws nothing, and takes no room.
struct StateIcon: View {
    @Environment(\.theme) var theme
    let state: String
    var large = false

    var body: some View {
        if let image = StateIconCache.image(state, size: large ? MasonTokens.sizeMascotIcon : MasonTokens.sizeMascotIconSmall) {
            let side = large ? theme.mascotIcon : theme.mascotIconSmall
            image.resizable().interpolation(.high)
                .frame(width: side, height: side)
                .accessibilityHidden(true)
        }
    }
}

/// State icons are cached by `Mascot` itself, so this is a thin alias kept for the call sites.
@MainActor enum StateIconCache {
    static func image(_ state: String, size: Int) -> Image? { Mascot.icon(state, size: size) }
}

/// The colour of a tile's number: the attention colour when the state wants the owner, gray
/// when the count is zero, else the text colour. The dash shown while disconnected is plain text.
func tileCountColor(_ theme: Theme, count: UInt32, wants: Bool, connected: Bool) -> Color {
    if wants { return theme.attention }
    return connected && count == 0 ? theme.muted : theme.text
}

/// A count tile: a large count over a one-line label, sized to its label so a
/// narrow window squeezes other text and never a tile or label; tinted when
/// the state wants the owner; a dash while disconnected.
struct CountTile: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let count: StateCount

    var body: some View {
        let wants = count.wantsYou && model.connected
        // Proposed and stuck fill only while one of their tasks is the one shown; the rest fill
        // whenever they want the owner.
        let filled = wants && (!Self.fillsOnlyWhenShown(count.state) || model.tileShowsCurrentTask(count))
        VStack(spacing: 0) {
            Text(model.tileCount(count)).font(theme.count()).foregroundStyle(tileCountColor(theme, count: count.count, wants: wants, connected: model.connected))
            Text(label).font(theme.small()).foregroundStyle(theme.muted).lineLimit(1).fixedSize()
        }
        .padding(.horizontal, theme.spacing)
        .frame(minWidth: theme.tileMinWidth, minHeight: theme.tileHeight, maxHeight: theme.tileHeight)
        .fixedSize(horizontal: true, vertical: false)
        .background(RoundedRectangle(cornerRadius: theme.radius).fill(filled ? theme.attentionTint : theme.surface))
        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(wants ? theme.attention : theme.border, lineWidth: 1))
        // An overlay, not part of the layout: Part 17 sizes a tile by its label and text size
        // alone, so a count of any width and the icon never change it.
        .overlay(alignment: .topLeading) {
            StateIcon(state: count.state)
                .offset(x: -theme.spacingSmall, y: -theme.spacingSmall)
                .allowsHitTesting(false)
        }
        .contentShape(Rectangle())
        .modifier(TileTap(model: model, state: count.state))
        .hint(model.tileTooltip(count))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(model.tileCount(count)) \(label)")
        .accessibilityIdentifier("tile:\(count.state)")
        .accessibilityAddTraits(.isButton)
    }

    var label: String { model.tileLabel(count) }

    static func fillsOnlyWhenShown(_ state: String) -> Bool { state == "proposed" || state == "needs_attention" }
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
            .touchTarget()
    }
}

extension View {
    func pill(prominent: Bool = false) -> some View { modifier(Pill(prominent: prominent)) }
}

public func copyToPasteboard(_ s: String) {
    #if canImport(AppKit)
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(s, forType: .string)
    #else
    UIPasteboard.general.string = s
    #endif
}

/// What a tile does with a click or tap that landed `clicks` times in a row (1 single, 2 double).
/// Both platforms go through this, so the click counts reaching `tileClicked` are the same.
@MainActor
func tileTapped(model: MonitorModel, state: String, clicks: Int) {
    model.tileClicked(state: state, clickCount: clicks)
}

/// Mac: one tap handler, the click count read from the current event. iOS: a double tap
/// (taking priority) and a single tap.
struct TileTap: ViewModifier {
    let model: MonitorModel
    let state: String

    func body(content: Content) -> some View {
        #if canImport(AppKit)
        content.onTapGesture {
            tileTapped(model: model, state: state, clicks: NSApp.currentEvent?.clickCount ?? 1)
        }
        #else
        content
            .gesture(TapGesture(count: 2).onEnded { tileTapped(model: model, state: state, clicks: 2) })
            .simultaneousGesture(TapGesture(count: 1).onEnded { tileTapped(model: model, state: state, clicks: 1) })
        #endif
    }
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
        .buttonStyle(.plain).touchTarget()
        .foregroundStyle(theme.muted)
        .hint("Copy \(value)")
    }
}

/// One labelled copy button on the task detail page.
struct CopyTarget: Equatable {
    let label: String
    let value: String

    /// Branch name then worktree, each only when the daemon sent a value.
    static func detail(branch: String, worktree: String) -> [CopyTarget] {
        var t: [CopyTarget] = []
        if !branch.isEmpty { t.append(CopyTarget(label: "Copy branch name", value: branch)) }
        if !worktree.isEmpty { t.append(CopyTarget(label: "Copy worktree", value: worktree)) }
        return t
    }

    /// What the button reads: its label, or "Copied" for a moment after a click.
    func shown(copied: Bool) -> String { copied ? "Copied" : label }
}

/// A bordered, labelled button that copies its value and says "Copied" for a moment.
struct CopyValueButton: View {
    @Environment(\.theme) var theme
    let target: CopyTarget
    let copy: (String) -> Void
    @State private var copied = false
    var body: some View {
        Button {
            copy(target.value)
            copied = true
            Task { @MainActor in
                try? await Task.sleep(nanoseconds: 1_500_000_000)
                copied = false
            }
        } label: {
            HStack(spacing: theme.spacingSmall) {
                Image(systemName: "doc.on.doc")
                Text(target.shown(copied: copied))
            }
            .font(theme.small())
        }
        .buttonStyle(.bordered)
        .hint(target.value)
        .accessibilityIdentifier("copy:\(target.label)")
    }
}

/// The decision card (Part 10): the ask, why only the owner can settle it, 2 to
/// 4 options with one recommended, "Other" with a text box, and the evidence.
/// An answer carries the card's hash, so a late click on a stale card is refused.
struct DecisionCardView: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let task: UInt64
    let card: Mason_V1_DecisionCard
    @State private var other = ""
    @State private var selection = OptionSelection(count: 0)
    @State private var multi = MultiSelection(count: 0)
    @FocusState private var cardFocused: Bool
    @FocusState private var otherFocused: Bool

    private func sendOther() {
        guard let text = otherAnswer(other) else { return }
        model.answer(task: task, text, cardHash: card.hash)
        other = ""
    }

    private func sendMulti() {
        guard let a = multi.answer else { return }
        model.answer(task: task, a, cardHash: card.hash)
    }

    private func resetSelection() {
        let rec = card.options.firstIndex { $0.recommended }
        selection = OptionSelection(count: card.options.count, recommended: rec)
        multi = MultiSelection(count: card.options.count, recommended: rec)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            HStack(alignment: .firstTextBaseline, spacing: theme.spacingSmall) {
                StateIcon(state: "needs_attention")
                LinkedText(model: model, text: card.ask, font: theme.title()).fixedSize(horizontal: false, vertical: true)
            }
            if !card.why.isEmpty {
                LinkedText(model: model, text: card.why, color: theme.muted).fixedSize(horizontal: false, vertical: true)
            }
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                ForEach(Array(card.options.enumerated()), id: \.offset) { i, o in
                    if model.canAct && card.multiSelect {
                        // Several answers: a checkbox per option; Send sends the checked ones.
                        Button { multi.toggle(i) } label: {
                            HStack {
                                Image(systemName: multi.checked.contains(i) ? "checkmark.square.fill" : "square")
                                optionLabel(i, o)
                            }
                            .pill(prominent: o.recommended)
                            .overlay(RoundedRectangle(cornerRadius: theme.radius)
                                .stroke(theme.text, lineWidth: multi.index == i ? 2 : 0))
                        }
                        .buttonStyle(.plain)
                        .accessibilityIdentifier("card:\(task):option:\(i + 1)")
                    } else if model.canAct {
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
                if model.canAct && card.multiSelect {
                    Button("Send the checked options") { sendMulti() }
                        .disabled(multi.answer == nil)
                        .accessibilityIdentifier("card:\(task):send-checked")
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
        .onKeyPress(.upArrow) {
            guard model.canAct else { return .ignored }
            if card.multiSelect { multi.up() } else { selection.up() }
            return .handled
        }
        .onKeyPress(.downArrow) {
            guard model.canAct else { return .ignored }
            if card.multiSelect { multi.down() } else { selection.down() }
            return .handled
        }
        .onKeyPress(.space) {
            guard model.canAct, card.multiSelect else { return .ignored }
            multi.toggle()
            return .handled
        }
        .onKeyPress(.return) {
            guard model.canAct else { return .ignored }
            if card.multiSelect { sendMulti(); return multi.answer == nil ? .ignored : .handled }
            guard let a = selection.answer else { return .ignored }
            model.answer(task: task, a, cardHash: card.hash)
            return .handled
        }
        .onAppear { resetSelection() }
        .onChange(of: card.hash) { _, _ in resetSelection() }
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
    let model: MonitorModel
    let task: UInt64
    let approval: Mason_V1_Approval
    @State private var reason = ""
    @State private var rejecting = false
    @State private var priority = MonitorModel.defaultApprovalPriority

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            LinkedText(model: model, text: approval.waitingFor, font: theme.title())
                .fixedSize(horizontal: false, vertical: true)
            ForEach(Array(approval.lines.enumerated()), id: \.offset) { _, l in
                LinkedText(model: model, text: "• \(l)")
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
                    Button { model.approve(task, approval, priority: priority) } label: { PendingLabel(text: "Approve", pending: model.pendingCommand(task: task, action: "approve"), prominent: true) }
                        .buttonStyle(.plain)
                        .disabled(model.isBusy(task: task))
                        .accessibilityIdentifier("approval:\(task):approve")
                    if approval.diffHash.isEmpty {
                        Picker("Priority", selection: $priority) {
                            ForEach(MonitorModel.approvalPriorities, id: \.self) { Text($0).tag($0) }
                        }
                        .pickerStyle(.menu).fixedSize()
                        .accessibilityIdentifier("approval:\(task):priority")
                        Button { model.deferTask(task) } label: { PendingLabel(text: "Defer", pending: model.pendingCommand(task: task, action: "defer")) }
                            .buttonStyle(.plain)
                            .disabled(model.isBusy(task: task))
                            .accessibilityIdentifier("approval:\(task):defer")
                    }
                    Button { rejecting.toggle() } label: { PendingLabel(text: "Reject", pending: model.pendingCommand(task: task, action: "reject")) }
                        .buttonStyle(.plain)
                        .disabled(model.isBusy(task: task))
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
            if let refused = model.actionError(task: task) ?? model.lastResult {
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
    @Environment(\.features) var features
    let model: MonitorModel
    let row: TaskRow
    var hasApprovalView = false

    /// The daemon's actions this platform may show: with agent sessions off there is no Open session.
    static func offered(_ actions: [String], features: PlatformFeatures) -> [String] {
        actions.filter { features.allows(action: $0) }
    }

    var body: some View {
        // One row when it fits; on a phone, too many buttons for the width stack in a column.
        ViewThatFits(in: .horizontal) {
            HStack(spacing: theme.spacing) { buttons }
            VStack(alignment: .leading, spacing: theme.spacing) { buttons }
        }
        .alert(OpenSession.confirmQuestion, isPresented: Binding(
            get: { model.sessionPrompt == row.id },
            set: { if !$0 { model.answerSessionPrompt(false) } }
        )) {
            Button("Open anyway") { model.answerSessionPrompt(true) }
            Button("Cancel", role: .cancel) {}
        }
    }

    /// Approve and Reject have their own view (ApprovalView) wherever there is one; a
    /// non-owner sees no buttons at all for what only the owner may do.
    @ViewBuilder var buttons: some View {
        ForEach(Self.offered(row.allowedActions, features: features).filter { shown($0) }, id: \.self) { a in
            Button { run(a) } label: { PendingLabel(text: label(a), pending: model.pendingCommand(task: row.id, action: a), prominent: a == "approve") }
                .buttonStyle(.plain).touchTarget()
                .disabled(model.isBusy(task: row.id))
                .accessibilityIdentifier("action:\(row.id):\(a)")
        }
        if let why = model.actionError(task: row.id) {
            Text(why).font(theme.small()).foregroundStyle(theme.attention)
                .accessibilityIdentifier("action:\(row.id):error")
        }
        if let notice = model.sessionNotice {
            Text(notice).font(theme.small()).foregroundStyle(theme.attention)
                .accessibilityIdentifier("action:\(row.id):open_session:notice")
        }
    }

    func shown(_ a: String) -> Bool {
        // Block and unblock are drawn by BlockersView, one control per blocker.
        if a == "answer" || a == "edit" || a == "block" || a == "unblock" { return false }
        if hasApprovalView && (a == "approve" || a == "reject") { return false }
        if !model.canAct && ["approve", "reject"].contains(a) { return false }
        return true
    }

    func label(_ a: String) -> String {
        switch a {
        case "skip_test": return "Skip test"
        case "skip_all_tests": return "Skip all testing"
        case "land_now": return "Land now"
        case "explore_anyway": return "Explore anyway"
        case "open_session": return "Open session"
        default: return a.prefix(1).uppercased() + a.dropFirst()
        }
    }

    func run(_ a: String) {
        switch a {
        case "cancel": model.cancel(row.id)
        case "retry": model.retry(row.id, seen: row.lastRun)
        case "approve": model.approve(row.id, priority: MonitorModel.defaultApprovalPriority)
        case "revive": model.revive(row.id)
        case "reject": model.reject(row.id)
        case "defer": model.deferTask(row.id)
        case "undefer": model.undefer(row.id)
        case "explore_anyway": model.exploreAnyway(row.id)
        case "land_now": model.landNow(row.id)
        case "force": model.force(row.id)
        case "skip_test": model.skipTest(row.id, step: row.step)
        case "skip_all_tests": model.makeTestImmune(row.id)
        case "open_session": model.openSession(row.id)
        default: break
        }
    }
}

/// A held task's blockers: each with its state and where it came from ("from GitHub" or "set
/// by you"), Release (and Revive for a canceled one, Retry for a failed one) for the owner, and
/// an Add blocker field with a task picker. A non-owner sees the list and no controls.
struct BlockersView: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let row: TaskRow
    @State private var pick: UInt64 = 0

    var body: some View {
        if !row.blockerLinks.isEmpty || row.allowedActions.contains("block") {
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                ForEach(row.blockerLinks, id: \.task) { l in
                    HStack(spacing: theme.spacing) {
                        TaskLink(title: "#\(l.task)", font: theme.body()) { model.go(.task(l.task)) }
                        Text("\(l.state), \(l.origin)").font(theme.small()).foregroundStyle(theme.muted)
                        if model.canAct {
                            ForEach(l.actions, id: \.self) { a in
                                Button { run(l, a) } label: { PendingLabel(text: label(a), pending: model.pendingCommand(task: l.task, action: a)) }
                                    .buttonStyle(.plain)
                                    .disabled(model.isBusy(task: row.id) || model.isBusy(task: l.task))
                                    .accessibilityIdentifier("blocker:\(row.id):\(l.task):\(a)")
                            }
                        }
                    }
                    .accessibilityElement(children: .contain)
                    .accessibilityIdentifier("blocker:\(row.id):\(l.task)")
                }
                if model.canAct && row.allowedActions.contains("block") {
                    HStack(spacing: theme.spacing) {
                        Picker("Add blocker", selection: $pick) {
                            Text("Choose a task").tag(UInt64(0))
                            ForEach(model.blockerCandidates(for: row), id: \.id) { c in
                                Text("\(c.key) \(c.title)").tag(c.id)
                            }
                        }
                        .accessibilityIdentifier("add-blocker:\(row.id):picker")
                        Button {
                            if pick != 0 { model.addBlocker(row.id, after: pick); pick = 0 }
                        } label: { Text("Add blocker").pill() }
                            .buttonStyle(.plain)
                            .disabled(pick == 0 || model.isBusy(task: row.id))
                            .accessibilityIdentifier("add-blocker:\(row.id)")
                    }
                }
            }
        }
    }

    func label(_ a: String) -> String { a.prefix(1).uppercased() + a.dropFirst() }

    func run(_ l: Mason_V1_BlockerLink, _ a: String) {
        switch a {
        case "release": model.release(row.id, blocker: l.task)
        case "revive": model.revive(l.task)
        case "retry": model.retry(l.task)
        default: break
        }
    }
}

/// A proposal or an improvement: who noticed it, how bad, where it goes, the proof, and (for an
/// improvement without write access) what would go upstream. Edit is offered only while it waits.
struct ProposalView: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
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
                LinkedText(model: model, text: "Filed by the agent of task #\(info.filedBy)", font: theme.small(), color: theme.muted)
            }
            if !info.evidence.isEmpty {
                Text("Evidence").font(theme.body(.semibold)).foregroundStyle(theme.text)
                LinkedText(model: model, text: info.evidence, font: theme.mono(), selectable: true)
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
    let model: MonitorModel
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
        .sheetWidth(520)
    }
}

/// "Improve the pipeline": type what MASON should change about itself.
struct ImproveSheet: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
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
        .sheetWidth(480)
    }
}

/// After a refused push: the keys the daemon could use, with what is known about each. Choosing
/// one writes the repository's ssh command, unlocks the key if it has a passphrase (in a dialog
/// on the daemon's Mac, never here) and tries the remote.
struct SSHKeySheet: View {
    @Environment(\.theme) var theme
    let model: MonitorModel

    var body: some View {
        let project = model.sshKeyProject ?? ""
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text("Choose the SSH key for \(project)").font(theme.title())
            if let keys = model.sshKeys {
                if keys.keys.isEmpty { Text("No keys were found in ~/.ssh on the daemon's Mac.").foregroundStyle(theme.muted) }
                ForEach(keys.keys) { k in
                    HStack(alignment: .firstTextBaseline, spacing: theme.spacing) {
                        if k.encrypted { Image(systemName: "lock.fill").accessibilityLabel("has a passphrase") }
                        VStack(alignment: .leading) {
                            Text(k.path.isEmpty ? "(only in the agent)" : k.path).font(theme.body(.medium))
                            Text("\(k.fingerprint)  \(k.comment)").font(theme.small()).foregroundStyle(theme.muted)
                        }
                        if k.onOwnerAccount { Text("on the repository owner's account").font(theme.small()).foregroundStyle(theme.accent) }
                        if let u = k.seenAs { Text("seen as \(u)").font(theme.small()).foregroundStyle(theme.muted) }
                        Spacer()
                        Button("Use") { model.useSSHKey(project: project, path: k.path) }
                            .disabled(!k.usable || model.sshKeyBusy)
                            .accessibilityIdentifier("ssh-key-use:\(k.id)")
                    }
                }
                if model.sshKeyBusy {
                    Text("Enter the passphrase in the dialog on \(keys.host.isEmpty ? "the daemon's Mac" : keys.host) if it asks.").font(theme.small())
                }
            } else if model.sshKeyResult == nil {
                Text("Looking for keys…").foregroundStyle(theme.muted)
            }
            if let r = model.sshKeyResult { Text(r).font(theme.small()).foregroundStyle(theme.attention) }
            HStack {
                Spacer()
                Button("Close") { model.closeSSHKeys() }.keyboardShortcut(.cancelAction)
            }
        }
        .padding(theme.spacingLarge)
        .frame(minWidth: 520)
        .accessibilityIdentifier("ssh-key-sheet")
    }
}

/// The pointing hand over a view, pushed on hover and popped on leaving or disappearing, balanced
/// the way the split-bar handle's cursor is (`Root.swift`). There is no hover on touch screens.
struct PointingHand: ViewModifier {
    @State private var inside = false

    func body(content: Content) -> some View {
        #if canImport(AppKit)
        content
            .onHover { over in
                if over && !inside { NSCursor.pointingHand.push() }
                if !over && inside { NSCursor.pop() }
                inside = over
            }
            .onDisappear { if inside { NSCursor.pop(); inside = false } }
        #else
        content
        #endif
    }
}

/// The one look of a link: the link colour (`theme.link`), underlined, and the pointing hand on
/// hover. Everything that opens a task's page or the tracker uses this, so a new link cannot forget
/// a part of it.
struct LinkStyle: ViewModifier {
    @Environment(\.theme) var theme

    func body(content: Content) -> some View {
        content.foregroundStyle(theme.link).underline().modifier(PointingHand())
    }
}

/// Only the colour and underline, for the title inside a row that is clickable as a whole (the row
/// itself takes `pointingHand()`).
struct LinkLook: ViewModifier {
    @Environment(\.theme) var theme

    func body(content: Content) -> some View {
        content.foregroundStyle(theme.link).underline()
    }
}

extension View {
    /// Link colour, underline and pointing hand.
    func linkStyle() -> some View { modifier(LinkStyle()) }
    /// Link colour and underline, no cursor of its own.
    func linkLook() -> some View { modifier(LinkLook()) }
    /// The pointing hand on hover.
    func pointingHand() -> some View { modifier(PointingHand()) }
}

/// A reference that opens a task's page in the monitor.
struct TaskLink: View {
    let title: String
    var font: Font? = nil
    let action: () -> Void

    var body: some View {
        Button(action: action) { Text(title).linkStyle() }
            .buttonStyle(.plain)
            .font(font)
    }
}

/// "label #a, #b, #c" as one wrapping `Text`, each task a link, so a long list wraps by whole
/// links and never splits a number. With `limit`, only that many are named and "and N more"
/// opens `moreTask`'s page. Never an `HStack` of separate texts (task 1020).
struct BlockerLine: View {
    @Environment(\.theme) var theme
    let label: String
    let ids: [UInt64]
    let font: Font
    var limit: Int? = nil
    var moreTask: UInt64 = 0
    let go: (UInt64) -> Void

    static func attributed(label: String, ids: [UInt64], limit: Int?, moreTask: UInt64,
                           muted: Color, link: Color) -> AttributedString {
        var out = AttributedString(label + " ")
        out.foregroundColor = muted
        let shown = limit.map { Array(ids.prefix($0)) } ?? ids
        for (i, id) in shown.enumerated() {
            var a = AttributedString("#\(id)")
            a.link = URL(string: "\(LinkedText.taskScheme)://\(id)")
            a.foregroundColor = link
            a.underlineStyle = .single
            out += a
            if i < shown.count - 1 { var c = AttributedString(", "); c.foregroundColor = muted; out += c }
        }
        if shown.count < ids.count {
            var and = AttributedString(" and "); and.foregroundColor = muted
            var more = AttributedString("\(ids.count - shown.count) more")
            more.link = URL(string: "\(LinkedText.taskScheme)://\(moreTask)")
            more.foregroundColor = link
            more.underlineStyle = .single
            out += and + more
        }
        return out
    }

    var body: some View {
        Text(Self.attributed(label: label, ids: ids, limit: limit, moreTask: moreTask,
                             muted: theme.muted, link: theme.link))
            .font(font)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .environment(\.openURL, OpenURLAction { url in
                if url.scheme == LinkedText.taskScheme, let n = UInt64(url.host ?? "") {
                    go(n)
                    return .handled
                }
                return .discarded
            })
    }
}

/// A reference that opens a web page (the tracker, a CI run) in the default browser.
struct WebLink: View {
    @Environment(\.openURL) private var openURL
    let title: String
    let url: URL
    var font: Font? = nil

    var body: some View {
        Button { openURL(url) } label: { Text(title).linkStyle() }
            .buttonStyle(.plain)
            .font(font)
    }
}

/// Text whose task numbers and CI references are links (`Linkify`): a task opens its page in the
/// monitor (so back and forward work), a CI reference opens in the default browser. Selection is
/// kept when `selectable`.
struct LinkedText: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let text: String
    var font: Font? = nil
    var color: Color? = nil
    var selectable = false
    /// Already styled text (the plan's Markdown): its own styling is kept and links are added on top.
    var base: AttributedString? = nil

    static let taskScheme = "mason-task"

    /// The text as an attributed string: one `link` run per task or URL segment.
    static func attributed(_ segments: [LinkSegment], linkColor: Color) -> AttributedString {
        var out = AttributedString()
        for s in segments {
            var a = AttributedString(s.text)
            switch s {
            case .text: break
            case .task(let id, _):
                a.link = URL(string: "\(taskScheme)://\(id)")
                a.foregroundColor = linkColor
                a.underlineStyle = .single
            case .url(let u, _):
                if let url = URL(string: u) {
                    a.link = url
                    a.foregroundColor = linkColor
                    a.underlineStyle = .single
                }
            }
            out += a
        }
        return out
    }

    /// `base` with a `link` on the part of it each task or URL segment covers.
    static func link(_ base: AttributedString, _ segments: [LinkSegment], linkColor: Color) -> AttributedString {
        var out = base
        var pos = 0
        for s in segments {
            let n = s.text.count
            defer { pos += n }
            var target: URL?
            switch s {
            case .text: continue
            case .task(let id, _): target = URL(string: "\(taskScheme)://\(id)")
            case .url(let u, _): target = URL(string: u)
            }
            guard let target else { continue }
            let start = out.characters.index(out.startIndex, offsetBy: pos)
            let end = out.characters.index(start, offsetBy: n)
            out[start..<end].link = target
            out[start..<end].foregroundColor = linkColor
            out[start..<end].underlineStyle = .single
        }
        return out
    }

    private var styled: Text {
        if let base {
            return Text(Self.link(base, model.linkify(String(base.characters)), linkColor: theme.link))
        }
        return Text(Self.attributed(model.linkify(text), linkColor: theme.link))
    }

    var body: some View {
        let t = styled.font(font ?? theme.body()).foregroundStyle(color ?? theme.text)
            .environment(\.openURL, OpenURLAction { url in
                if url.scheme == Self.taskScheme, let n = UInt64(url.host ?? "") {
                    model.go(.task(n))
                    return .handled
                }
                guard let s = url.scheme?.lowercased(), s == "https" || s == "http" else { return .discarded }
                return .systemAction
            })
        if selectable { t.textSelection(.enabled) } else { t.textSelection(.disabled) }
    }
}

/// The "CI" line and the tracker issue of a task's page: each reference a link.
struct CiLinksLine: View {
    @Environment(\.theme) var theme
    let links: [Mason_V1_CiLink]
    let issue: UInt64
    let issueURL: String

    var body: some View {
        HStack(spacing: theme.spacingSmall) {
            if issue != 0 {
                if Linkify.isWebURL(issueURL), let u = URL(string: issueURL) {
                    WebLink(title: "issue #\(issue)", url: u, font: theme.body())
                        .accessibilityIdentifier("detail:issue")
                } else {
                    Text("issue #\(issue)").font(theme.body()).foregroundStyle(theme.muted)
                }
            }
            let shown = links.filter { Linkify.isWebURL($0.url) }
            if !shown.isEmpty {
                Text("CI").font(theme.body()).foregroundStyle(theme.muted)
                ForEach(Array(shown.enumerated()), id: \.offset) { _, l in
                    if let u = URL(string: l.url) {
                        WebLink(title: l.label.isEmpty ? l.url : l.label, url: u, font: theme.body())
                    }
                }
            }
        }
        .accessibilityIdentifier("detail:ci")
    }
}

/// One one-second tick for every progress line on the page, in place of a timer per row. It runs
/// only while some line is on screen and the app is visible: hidden or minimised, it stops.
@MainActor
final class SharedTick: ObservableObject {
    static let shared = SharedTick()
    @Published private(set) var now = Date()
    private var users = 0
    private var visible = true
    private var timer: Timer?

    private init() {
        #if canImport(AppKit)
        NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeOcclusionStateNotification, object: nil, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                let v = NSApp.occlusionState.contains(.visible)
                self?.model?.visible = v
                self?.visible = v
                self?.update()
            }
        }
        #endif
    }

    /// iOS: the root view reports the scene phase here (active means visible).
    /// The model that is told the same thing (task #973): one notion of "visible".
    weak var model: MonitorModel?

    func setVisible(_ v: Bool) {
        model?.visible = v
        guard visible != v else { return }
        visible = v
        update()
    }

    func join() { users += 1; update() }
    func leave() { users = max(0, users - 1); update() }

    private func update() {
        if users > 0 && visible {
            guard timer == nil else { return }
            now = Date()
            timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in
                MainActor.assumeIsolated { self?.now = Date() }
            }
        } else {
            timer?.invalidate()
            timer = nil
        }
    }
}

extension View {
    /// A sheet's width: fixed on the Mac, where a sheet is as wide as its content says; on iOS it
    /// is at most this wide and never wider than the screen (a phone is narrower than 460 pt).
    @ViewBuilder func sheetWidth(_ width: CGFloat) -> some View {
        #if os(macOS)
        frame(width: width)
        #else
        frame(maxWidth: width)
        #endif
    }
}


/// The cause of a wait as a small badge on a task row (s527).
struct WaitBadge: View {
    @Environment(\.theme) var theme
    let cause: String

    var body: some View {
        Text(QueueWait.badge(cause))
            .font(theme.small())
            .padding(.horizontal, 6).padding(.vertical, 1)
            .background(Capsule().fill(theme.attention.opacity(0.15)))
            .foregroundStyle(theme.attention)
            .accessibilityIdentifier("wait:badge")
    }
}

/// One line at the top of home when most waiting work is held by one request, one gate or one
/// red test: what holds it, for how long, and how many tasks wait behind it (s527).
struct QueueHoldBanner: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let hold: Mason_V1_QueueHold

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            LinkedText(model: model, text: hold.summary, font: theme.body(.semibold), color: theme.attention)
                .accessibilityIdentifier("home:queue-hold")
            if hold.hasParty && hold.party.lease {
                LinkedText(model: model, text: QueueWait.lease(hold.party, now: model.daemonNow()),
                           font: theme.small(), color: theme.muted)
                    .accessibilityIdentifier("home:queue-hold:lease")
            }
            ForEach(hold.tasks.filter { _ in hold.kind == "red" }.prefix(8), id: \.self) { t in
                TaskLink(title: "#\(t)") { model.go(.task(t)) }
            }
        }
        .padding(theme.spacing)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RoundedRectangle(cornerRadius: 6).fill(theme.attention.opacity(0.12)))
    }
}

/// A pill that says what is happening while its command is pending: a working label and a small
/// progress indicator while it is sent, the accepted label once masond said yes (task #522).
struct PendingLabel: View {
    let text: String
    let pending: PendingCommand?
    var prominent = false

    var body: some View {
        HStack(spacing: 4) {
            if pending?.stage == .sending { ProgressView().controlSize(.small) }
            Text(pending?.label ?? text)
        }
        .pill(prominent: prominent)

    }
}
