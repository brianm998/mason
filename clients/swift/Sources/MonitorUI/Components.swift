// SPDX-License-Identifier: GPL-3.0-or-later

import AppKit
import MasonProtocol
import MonitorCore
import SwiftUI

/// Progress cells (Part 17): filled means finished; part filled means a real
/// count (or the typical time when there is none); hatched means waiting,
/// labelled with what for; empty means not started. No percentages. A step
/// running past its typical time turns amber, never red: slow is not broken.
struct ProgressCells: View {
    @Environment(\.theme) var theme
    let cells: [Mason_V1_ProgressCell]
    var slow: Bool = false

    var body: some View {
        HStack(spacing: theme.spacingSmall) {
            ForEach(Array(cells.enumerated()), id: \.offset) { _, c in
                Cell(state: c.state, tint: slow && c.state == .part ? theme.slow : theme.accent)
                    .frame(width: theme.cell, height: theme.cell)
                    .help(c.label.isEmpty ? "\(c.step)" : "\(c.step): \(c.label)")
                    .accessibilityLabel("\(c.step), \(describe(c.state))")
            }
        }
    }

    private func describe(_ s: Mason_V1_CellState) -> String {
        switch s {
        case .filled: return "finished"
        case .part: return "running"
        case .hatched: return "waiting"
        default: return "not started"
        }
    }
}

struct Cell: View {
    @Environment(\.theme) var theme
    let state: Mason_V1_CellState
    let tint: Color

    var body: some View {
        let shape = RoundedRectangle(cornerRadius: 2)
        ZStack {
            switch state {
            case .filled:
                shape.fill(tint)
            case .part:
                shape.stroke(tint, lineWidth: 1)
                GeometryReader { g in
                    Rectangle().fill(tint).frame(width: g.size.width / 2).clipShape(shape)
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

/// A count tile: a large count over a one-line label, a fixed size so a
/// narrow window squeezes other text and never breaks a label; tinted when
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
        .frame(width: theme.tileWidth, height: theme.tileHeight)
        .background(RoundedRectangle(cornerRadius: theme.radius).fill(wants ? theme.attentionTint : theme.surface))
        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(wants ? theme.attention : theme.border, lineWidth: 1))
        .contentShape(Rectangle())
        .onTapGesture { model.reveal(state: count.state) }
        .help(count.tooltip)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(model.tileCount(count)) \(label)")
        .accessibilityIdentifier("tile:\(count.state)")
        .accessibilityAddTraits(.isButton)
    }

    var label: String { count.state.replacingOccurrences(of: "_", with: " ") }
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

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text(card.ask).font(theme.title()).foregroundStyle(theme.text).fixedSize(horizontal: false, vertical: true)
            if !card.why.isEmpty {
                Text(card.why).font(theme.body()).foregroundStyle(theme.muted).fixedSize(horizontal: false, vertical: true)
            }
            VStack(alignment: .leading, spacing: theme.spacingSmall) {
                ForEach(Array(card.options.enumerated()), id: \.offset) { i, o in
                    Button { model.answer(task: task, String(i + 1), cardHash: card.hash) } label: {
                        HStack {
                            Text("\(i + 1).")
                            Text(o.label)
                            if o.recommended {
                                Text("recommended").font(theme.small()).foregroundStyle(Color.white.opacity(0.85))
                            }
                            Spacer(minLength: 0)
                        }
                        .pill(prominent: o.recommended)
                    }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("card:\(task):option:\(i + 1)")
                }
                HStack {
                    TextField("Other…", text: $other)
                        .textFieldStyle(.roundedBorder)
                        .font(theme.body())
                        .accessibilityIdentifier("card:\(task):other")
                    Button("Send") {
                        model.answer(task: task, other, cardHash: card.hash)
                        other = ""
                    }
                    .disabled(other.trimmingCharacters(in: .whitespaces).isEmpty)
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
        }
        .padding(theme.spacingLarge)
        .background(RoundedRectangle(cornerRadius: theme.radius).fill(theme.attentionTint))
        .overlay(RoundedRectangle(cornerRadius: theme.radius).stroke(theme.attention, lineWidth: 1))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("card:\(task)")
    }
}

/// The actions a task offers now: exactly the daemon's `allowed_actions`.
struct ActionButtons: View {
    @Environment(\.theme) var theme
    @ObservedObject var model: MonitorModel
    let row: TaskRow

    var body: some View {
        HStack(spacing: theme.spacing) {
            ForEach(row.allowedActions.filter { $0 != "answer" && $0 != "edit" }, id: \.self) { a in
                Button { run(a) } label: { Text(label(a)).pill(prominent: a == "approve") }
                    .buttonStyle(.plain)
                    .accessibilityIdentifier("action:\(row.id):\(a)")
            }
        }
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
