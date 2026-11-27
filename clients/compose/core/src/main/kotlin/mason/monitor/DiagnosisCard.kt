// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/**
 * The logic of the diagnosis card and the context field (#1046), apart from any window so it is
 * tested without one. The SwiftUI client has the same rules (DiagnosisCard.swift).
 */

/** The context field's text, trimmed; null when it is blank (an empty context is not sent). */
fun contextText(text: String): String? = otherAnswer(text)

/** What a click sends from a decision card: the answer (an option's number or free text) and the context ("" when none). */
data class AnswerRequest(val answer: String, val context: String) {
    companion object {
        /** An option clicked, or Return on the selected one. [option] is 0-based. */
        fun option(option: Int, context: String) = AnswerRequest((option + 1).toString(), contextText(context) ?: "")

        /** The "Other" box: free text, with the context field's text as `context` when there is any. Null when the text is blank. */
        fun other(text: String, context: String): AnswerRequest? =
            otherAnswer(text)?.let { AnswerRequest(it, contextText(context) ?: "") }

        /** The checked options of a multi-select card (already "1,3"). */
        fun multi(answer: String, context: String) = AnswerRequest(answer, contextText(context) ?: "")
    }
}

/** What a failed task's page shows first. */
sealed class FailedPage {
    /** The diagnosis card, with the failure line under it. */
    object Card : FailedPage()
    /** A diagnosis is running: "Diagnosing on …", in place of the card. */
    object Diagnosing : FailedPage()
    /** No diagnosis: the buttons as before, plus Diagnose when it is offered. */
    data class Buttons(val diagnose: Boolean) : FailedPage()

    /** "Other actions" is collapsed by default behind a card or a running diagnosis; with no diagnosis the buttons are the page. */
    val otherActionsCollapsed: Boolean get() = this !is Buttons

    /** Whether [action] goes under "Other actions" for this presentation. */
    fun hidesUnderOther(action: String): Boolean = otherActionsCollapsed && action !in alwaysVisible

    companion object {
        val alwaysVisible = setOf("open_session", "diagnose", "answer")

        /** Chooses the presentation from what the task view carries. */
        fun choose(hasCard: Boolean, diagnosing: Boolean, actions: List<String>): FailedPage = when {
            hasCard -> Card
            diagnosing -> Diagnosing
            else -> Buttons("diagnose" in actions)
        }
    }
}

/** "Diagnosing on Opus medium… started 3 min ago". */
fun diagnosingLine(startedMs: Long, ranOn: String, now: Long): String {
    val on = if (ranOn.isEmpty()) "" else " on $ranOn"
    val ago = if (now > startedMs) "${Format.duration(now - startedMs)} ago" else "just now"
    return "Diagnosing$on… started $ago"
}

/** What a failed task's tile and row add next to its activity ("idle"): "diagnosis ready", "diagnosing", or nothing. */
fun diagnosisBadge(diagnosis: String): String? = when (diagnosis) {
    "ready" -> "diagnosis ready"
    "running" -> "diagnosing"
    else -> null
}
