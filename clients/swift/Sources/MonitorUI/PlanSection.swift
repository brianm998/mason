// SPDX-License-Identifier: GPL-3.0-or-later

import MasonProtocol
import MonitorCore
import SwiftUI

/// Inline Markdown (code, bold) in one text item. Block Markdown, links and images are not
/// rendered; if parsing fails the text is shown as written.
func inlineMarkdown(_ s: String) -> AttributedString {
    let opts = AttributedString.MarkdownParsingOptions(interpretedSyntax: .inlineOnlyPreservingWhitespace)
    return (try? AttributedString(markdown: s, options: opts)) ?? AttributedString(s)
}

/// A task's plan: summary, steps, files, criteria and any extra keys, as a document.
struct PlanSection: View {
    @Environment(\.theme) var theme
    let detail: Mason_V1_TaskDetailView

    var body: some View {
        if !detail.hasPlanView {
            // An older daemon: only the text.
            Text(detail.plan).font(theme.body()).textSelection(.enabled).frame(maxWidth: .infinity, alignment: .leading)
        } else {
            let v = detail.planView
            if !v.note.isEmpty {
                Text(v.note).font(theme.small()).foregroundStyle(theme.muted)
                Text(v.raw).font(theme.mono()).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .fixedSize(horizontal: false, vertical: true)
                HStack { CopyButton(label: "Copy JSON", value: v.raw) }
            } else {
                readable(v)
            }
        }
    }

    @ViewBuilder func readable(_ v: Mason_V1_PlanView) -> some View {
        let open = PlanText.expandedByDefault(stepCount: v.steps.count)
        VStack(alignment: .leading, spacing: theme.spacing) {
            HStack(spacing: theme.spacing) {
                Button("Copy plan") { copyToPasteboard(PlanText.copyAsText(v)) }.pill()
                    .buttonStyle(.plain).accessibilityIdentifier("plan-copy-text")
                CopyButton(label: "Copy JSON", value: v.raw)
            }
            ForEach(Array(v.summaryParagraphs.enumerated()), id: \.offset) { _, p in text(p) }
            if !v.steps.isEmpty || !v.lead.isEmpty {
                PlanGroup(title: v.stepsAreList ? "Steps (\(v.steps.count))" : "Approach", open: open) {
                    if !v.lead.isEmpty { text(v.lead) }
                    ForEach(Array(v.steps.enumerated()), id: \.offset) { i, step in
                        if let m = PlanText.marker(v, i) {
                            HStack(alignment: .firstTextBaseline, spacing: theme.spacingSmall) {
                                Text(m).font(theme.body()).foregroundStyle(theme.muted)
                                text(step)
                            }
                        } else {
                            text(step)
                        }
                    }
                }
            }
            if !v.files.isEmpty {
                PlanGroup(title: "Files (\(v.files.count))", open: open) {
                    ForEach(Array(v.files.enumerated()), id: \.offset) { _, f in
                        HStack(alignment: .firstTextBaseline, spacing: theme.spacingSmall) {
                            Text(f).font(theme.mono()).textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            Button { copyToPasteboard(f) } label: { Image(systemName: "doc.on.doc") }
                                .buttonStyle(.plain).foregroundStyle(theme.muted).help("Copy \(f)")
                        }
                    }
                }
            }
            if !v.criteria.isEmpty {
                PlanGroup(title: "Acceptance criteria (\(v.criteria.count))", open: open) {
                    ForEach(Array(v.criteria.enumerated()), id: \.offset) { _, c in
                        HStack(alignment: .firstTextBaseline, spacing: theme.spacingSmall) {
                            Text("☐").font(theme.body()).foregroundStyle(theme.muted)
                            text(c)
                        }
                    }
                }
            }
            if !v.other.isEmpty {
                PlanGroup(title: "Other", open: true) {
                    ForEach(Array(v.other.enumerated()), id: \.offset) { _, e in
                        Text(e.label).font(theme.body()).fontWeight(.semibold)
                        text(e.value)
                    }
                }
            }
        }
    }

    func text(_ s: String) -> some View {
        Text(inlineMarkdown(s)).font(theme.body()).textSelection(.enabled)
            .frame(maxWidth: .infinity, alignment: .leading)
            .fixedSize(horizontal: false, vertical: true)
    }
}

/// A disclosure group that starts open or closed.
struct PlanGroup<Content: View>: View {
    @Environment(\.theme) var theme
    let title: String
    @State var expanded: Bool
    let content: Content
    init(title: String, open: Bool, @ViewBuilder content: () -> Content) {
        self.title = title
        _expanded = State(initialValue: open)
        self.content = content()
    }
    var body: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: theme.spacingSmall) { content }
                .padding(.top, theme.spacingSmall)
        } label: {
            Text(title).font(theme.body()).fontWeight(.semibold)
        }
    }
}
