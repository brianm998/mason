// SPDX-License-Identifier: GPL-3.0-or-later

#if os(macOS)
import AppKit
import MonitorCore
import SwiftUI

/// Add Project: the wizard over the daemon's onboarding endpoints. Desktop only; the iOS app
/// shares this module and never builds it. The steps are Repository, Detected, Questions,
/// Review and Apply, as in the Compose desktop client.
struct AddProjectSheet: View {
    @Bindable var model: MonitorModel

    var body: some View {
        if let w = model.addProject {
            AddProjectContent(model: model, wizard: w)
        }
    }
}

/// The wizard's current step, drawn from a state value (so a test can draw any step).
struct AddProjectContent: View {
    @Environment(\.theme) var theme
    var model: MonitorModel
    var wizard: AddProjectWizard
    @State private var typed = ""
    @State private var expanded: Set<String> = []

    var body: some View {
        VStack(alignment: .leading, spacing: theme.spacing) {
            Text("Add project").font(theme.title())
            Text(wizard.step.rawValue).font(theme.body(.semibold)).foregroundStyle(theme.muted)
                .accessibilityIdentifier("add-project-step")
            Group {
                switch wizard.step {
                case .repository: repository
                case .detected: detected
                case .questions: questions
                case .review: review
                case .apply: apply
                }
            }
            if let why = wizard.refusal {
                Text(why).font(theme.small()).foregroundStyle(theme.attention).textSelection(.enabled)
                    .accessibilityIdentifier("add-project-refusal")
            }
            Spacer(minLength: 0)
            buttons
        }
        .font(theme.body())
        .padding(theme.spacingLarge)
        .frame(width: 560, height: 460, alignment: .topLeading)
    }

    // MARK: steps

    private var repository: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            Text("Pick a repository MASON found, or type the path of a git work tree on the daemon's machine.")
                .font(theme.small()).foregroundStyle(theme.muted)
            ScrollView {
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(wizard.candidates, id: \.path) { c in
                        Button { model.addProject?.path = c.path } label: {
                            HStack {
                                Image(systemName: wizard.path == c.path ? "largecircle.fill.circle" : "circle")
                                VStack(alignment: .leading) {
                                    Text(c.name).font(theme.body(.semibold))
                                    Text(c.path + (c.remote.map { "  ·  \($0)" } ?? "")).font(theme.small()).foregroundStyle(theme.muted)
                                }
                                Spacer()
                                if c.hasMason { Text("has .mason").font(theme.small()).foregroundStyle(theme.muted) }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                    if wizard.candidates.isEmpty {
                        Text("No candidates. Add folders to [onboarding] roots, or type a path.")
                            .font(theme.small()).foregroundStyle(theme.muted)
                    }
                    ForEach(wizard.candidateErrors, id: \.self) { Text($0).font(theme.small()).foregroundStyle(theme.attention) }
                }
            }
            .frame(maxHeight: 200)
            HStack {
                TextField("Path", text: Binding(get: { wizard.path }, set: { model.addProject?.path = $0 }))
                    .textFieldStyle(.roundedBorder).accessibilityIdentifier("add-project-path")
                if model.localDaemon {
                    Button("Choose…") { choose() }.accessibilityIdentifier("add-project-choose")
                }
            }
        }
    }

    private var detected: some View {
        let p = wizard.plan ?? OnboardingPlan()
        return VStack(alignment: .leading, spacing: theme.spacingSmall) {
            row("Project", p.id)
            row("Remote", p.remote.isEmpty ? "none" : p.remote)
            row("Base", p.base)
            row("Languages", p.languages.isEmpty ? "none found" : p.languages.joined(separator: ", "))
            row("CI", p.ci.isEmpty ? "none found" : p.ci.joined(separator: ", "))
            row("Agents", p.agents.isEmpty ? "none found" : p.agents.joined(separator: ", "))
            row("Setup", p.kind == "adopt" ? "Adopt: the repository already has .mason" : "Init: MASON writes a fresh setup")
        }
    }

    private var questions: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            if let q = wizard.question {
                Text(q.text).font(theme.body(.semibold))
                if q.isFreeText {
                    TextField(q.recommended.isEmpty ? "Answer" : q.recommended, text: $typed)
                        .textFieldStyle(.roundedBorder).accessibilityIdentifier("add-project-answer")
                } else {
                    ForEach(q.options, id: \.self) { o in
                        Button { typed = o } label: {
                            HStack {
                                Image(systemName: (typed.isEmpty ? q.recommended : typed) == o ? "largecircle.fill.circle" : "circle")
                                Text(o)
                                if o == q.recommended { Text("recommended").font(theme.small()).foregroundStyle(theme.muted) }
                            }
                        }
                        .buttonStyle(.plain)
                    }
                }
            } else {
                Text("Nothing more to ask.").foregroundStyle(theme.muted)
            }
        }
    }

    private var review: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            Text("MASON will write these files:").font(theme.small()).foregroundStyle(theme.muted)
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(wizard.plan?.files ?? [], id: \.path) { f in
                        DisclosureGroup(isExpanded: Binding(
                            get: { expanded.contains(f.path) },
                            set: { if $0 { expanded.insert(f.path) } else { expanded.remove(f.path) } })
                        ) {
                            Text(f.contents).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        } label: { Text(f.path) }
                    }
                }
            }
            .frame(maxHeight: 190)
            ForEach(wizard.plan?.warnings ?? [], id: \.self) {
                Text($0).font(theme.small()).foregroundStyle(theme.attention)
            }
            Picker("When setup is written, land it", selection: Binding(
                get: { wizard.land }, set: { model.addProject?.land = $0 })
            ) {
                Text("No: leave the branch").tag("no")
                Text("Merge into the base").tag("merge")
                Text("Merge and push").tag("push")
                Text("Open a pull request").tag("pr")
            }
            .accessibilityIdentifier("add-project-land")
            Toggle("File a proof task (a harmless first task)", isOn: Binding(
                get: { wizard.proveTask }, set: { model.addProject?.proveTask = $0 }))
                .accessibilityIdentifier("add-project-prove")
        }
    }

    private var apply: some View {
        VStack(alignment: .leading, spacing: theme.spacingSmall) {
            ScrollView {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(wizard.progress.enumerated()), id: \.offset) { _, l in
                        Text(l).font(.system(.caption, design: .monospaced)).textSelection(.enabled)
                    }
                    if wizard.busy { ProgressView().controlSize(.small) }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            if let id = wizard.appliedId {
                Text("Added \(id).").font(theme.body(.semibold))
                ForEach(wizard.warnings, id: \.self) { Text($0).font(theme.small()).foregroundStyle(theme.attention) }
                ForEach(wizard.notes, id: \.self) { Text($0).font(theme.small()).foregroundStyle(theme.attention) }
                if let t = wizard.proofTask {
                    Button("Open proof task #\(t)") { model.finishAddProject(openProofTask: true) }
                        .accessibilityIdentifier("add-project-proof")
                }
            }
        }
    }

    // MARK: buttons

    private var buttons: some View {
        HStack {
            Spacer()
            switch wizard.step {
            case .repository:
                Button("Cancel") { model.closeAddProject() }
                Button("Continue") { model.planAddProject() }
                    .keyboardShortcut(.defaultAction).disabled(!wizard.canPlan)
            case .detected:
                Button("Cancel") { model.closeAddProject() }
                Button("Back") { model.addProject?.back() }
                Button("Continue") { model.addProject?.continueFromDetected(); typed = "" }
                    .keyboardShortcut(.defaultAction)
            case .questions:
                Button("Cancel") { model.closeAddProject() }
                Button("Back") { model.addProject?.back(); typed = "" }
                Button("Use recommended for the rest") { typed = ""; model.addProjectUseRecommendedForRest() }
                    .disabled(wizard.question == nil || wizard.busy)
                Button("Next") {
                    if let q = wizard.question {
                        let v = typed.isEmpty ? q.recommended : typed
                        typed = ""
                        model.answerAddProject(v)
                    }
                }
                .keyboardShortcut(.defaultAction).disabled(wizard.busy)
            case .review:
                Button("Cancel") { model.closeAddProject() }
                Button("Back") { model.addProject?.back() }
                Button("Add project") { model.applyAddProject() }
                    .keyboardShortcut(.defaultAction)
                    .disabled(wizard.busy || !(wizard.plan?.errors.isEmpty ?? true))
            case .apply:
                if wizard.appliedId != nil {
                    Button("Done") { model.finishAddProject(openProofTask: false) }.keyboardShortcut(.defaultAction)
                }
            }
        }
    }

    private func row(_ k: String, _ v: String) -> some View {
        HStack(alignment: .top) {
            Text(k).foregroundStyle(theme.muted).frame(width: 90, alignment: .leading)
            Text(v).textSelection(.enabled)
        }
    }

    private func choose() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        if panel.runModal() == .OK, let u = panel.url { model.addProject?.path = u.path }
    }
}
#endif
