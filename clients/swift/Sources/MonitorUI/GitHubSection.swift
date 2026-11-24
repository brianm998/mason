// SPDX-License-Identifier: GPL-3.0-or-later

import MonitorCore
import SwiftUI

/// Home's Remote runs section (#767): every run MASON has on GitHub or on a node, full width under CPU and Disk.
/// The page (`mason://section/github`) adds every shard's job.
struct GitHubSection: View {
    @Environment(\.theme) var theme
    let model: MonitorModel
    let g: GithubRunList
    var open: (() -> Void)?
    var detail = false

    var body: some View {
        section("Remote runs", theme: theme, open: open) {
            Text(GitHubRuns.header(g)).font(theme.body()).foregroundStyle(theme.muted)
                .accessibilityIdentifier("github:header")
            if g.runs.isEmpty {
                Text(GitHubRuns.empty).font(theme.body()).foregroundStyle(theme.muted)
            }
            ForEach(g.runs, id: \.id) { r in
                VStack(alignment: .leading, spacing: theme.spacingSmall) {
                    HStack(spacing: theme.spacing) {
                        if r.task != 0 {
                            TaskLink(title: GitHubRuns.task(r), font: theme.body()) { model.go(.task(r.task)) }
                        } else {
                            Text(GitHubRuns.task(r)).font(theme.body(.semibold))
                        }
                        Text(r.taskState).font(theme.small()).foregroundStyle(theme.muted)
                        Text(GitHubRuns.kind(r)).font(theme.body())
                        Text(GitHubRuns.state(r)).font(theme.body(.semibold))
                        if let u = URL(string: r.url), Linkify.isWebURL(r.url) {
                            WebLink(title: "open on GitHub", url: u, font: theme.small())
                        }
                    }
                    Text([r.progress, "updated \(GitHubRuns.duration(r.updatedAgoS)) ago"]
                        .filter { !$0.isEmpty }.joined(separator: " · "))
                        .font(theme.small()).foregroundStyle(theme.muted)
                    if detail {
                        ForEach(Array(r.jobs.enumerated()), id: \.offset) { _, j in
                            Text(GitHubRuns.job(j)).font(theme.small()).foregroundStyle(theme.muted)
                                .padding(.leading, theme.spacingLarge)
                        }
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityIdentifier("github:\(r.id)")
            }
        }
    }
}
