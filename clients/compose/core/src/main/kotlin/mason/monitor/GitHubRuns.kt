// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.GithubJob
import mason.v1.GithubRun
import mason.v1.GithubRunList

/** The text of home's GitHub section: the daemon sends every number, this only words them, the same as GitHubRuns.swift. */
object GitHubRuns {
    const val EMPTY = "Nothing on GitHub."

    fun header(g: GithubRunList): String = "MASON ${g.pool_used} of ${g.pool_limit} runners, ${g.waiting} waiting"

    fun duration(s: Long): String = if (s < 90) "$s s" else "${s / 60} min"

    fun task(r: GithubRun): String = if (r.title.isEmpty()) r.owner else "${r.owner} ${r.title}"

    fun kind(r: GithubRun): String {
        var t = if (r.suite.isEmpty()) r.kind else "${r.kind} ${r.suite}"
        if (r.runs_on.isNotEmpty()) t += " on ${r.runs_on}"
        if (r.shards > 1) t += ", ${r.shards} shards"
        return t
    }

    fun state(r: GithubRun): String = when (r.state) {
        "waiting" -> "waiting for a runner, ${duration(r.elapsed_s)}"
        "queued" -> "queued on GitHub ${duration(r.elapsed_s)}"
        "running" -> "running ${duration(r.elapsed_s)}"
        else -> r.state
    }

    fun row(r: GithubRun): String {
        val parts = mutableListOf(task(r))
        if (r.task_state.isNotEmpty()) parts += r.task_state
        parts += kind(r)
        parts += state(r)
        if (r.progress.isNotEmpty()) parts += r.progress
        parts += "updated ${duration(r.updated_ago_s)} ago"
        return parts.joinToString(" · ")
    }

    fun job(j: GithubJob): String {
        var t = "${j.name}: ${j.state}"
        if (j.conclusion.isNotEmpty()) t += " (${j.conclusion})"
        if (j.step.isNotEmpty()) t += ", step ${j.step}"
        return t
    }
}
