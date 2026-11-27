// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import org.json.JSONObject

/** The wizard's five steps. The names are the Swift monitor's. */
enum class AddProjectStep(val title: String) {
    REPOSITORY("Repository"), DETECTED("Detected"), QUESTIONS("Questions"), REVIEW("Review"), APPLY("Apply"),
}

/** A git repository the daemon offers (`GET /onboarding/candidates`). */
data class Candidate(val path: String, val name: String, val hasMason: Boolean, val remote: String?)

/** One question of `mason init`, as the daemon's plan names it. */
data class WizardQuestion(val id: String, val text: String, val options: List<String>, val default: String) {
    /** The key of this answer in the `answers` map: the flag's field name. */
    val key: String get() = id.replace('-', '_')
}

data class WizardFile(val path: String, val contents: String)

/** The daemon's reply to `POST /onboarding/plan`. */
data class WizardPlan(
    val detected: List<Pair<String, String>>,
    val kind: String,
    val next: WizardQuestion?,
    val files: List<WizardFile>,
    val errors: List<String>,
    val warnings: List<String> = emptyList(),
) {
    companion object {
        fun parse(json: String): WizardPlan? {
            val o = try { JSONObject(json) } catch (e: Exception) { return null }
            val d = o.optJSONObject("detected")
            val detected = mutableListOf<Pair<String, String>>()
            if (d != null) {
                fun text(v: Any?): String = when (v) {
                    null, JSONObject.NULL -> ""
                    is org.json.JSONArray -> (0 until v.length()).joinToString(", ") { text(v.get(it)) }
                    is JSONObject -> v.keys().asSequence().joinToString(" ") { k -> text(v.get(k)) }
                    else -> v.toString()
                }
                for ((label, key) in listOf("Repository" to "root", "Name" to "id", "Branch" to "branch", "Languages" to "languages", "CI" to "ci", "Remote" to "remote")) {
                    val t = text(d.opt(key))
                    if (t.isNotEmpty()) detected += label to t
                }
                if (d.optBoolean("existing_mason")) detected += "MASON" to "already set up here (adopt)"
            }
            val n = o.optJSONObject("next")?.let {
                WizardQuestion(
                    it.optString("id"), it.optString("text"),
                    EditorJson.strings(it.optJSONArray("options")), it.optString("default"),
                )
            }
            val files = o.optJSONArray("files")?.let { a ->
                (0 until a.length()).map { i -> a.getJSONObject(i).let { WizardFile(it.optString("path"), it.optString("contents")) } }
            } ?: emptyList()
            return WizardPlan(detected, o.optString("kind"), n, files, EditorJson.strings(o.optJSONArray("errors")), EditorJson.strings(o.optJSONArray("warnings")))
        }
    }
}

/** The landing choices, as the CLI offers them; "no" is the default. */
val landChoices = listOf("no", "merge", "push", "pr")

/**
 * Add Project: the state of the wizard, and nothing else. The monitor sends the plan and apply
 * calls ([MonitorModel.addProjectNext] and friends) and feeds the answers back in here.
 */
class AddProjectWizard {
    var step by mutableStateOf(AddProjectStep.REPOSITORY)
        private set
    var path by mutableStateOf("")
    var candidates by mutableStateOf<List<Candidate>>(emptyList())
        private set
    var candidateErrors by mutableStateOf<List<String>>(emptyList())
        private set
    var plan by mutableStateOf<WizardPlan?>(null)
        private set
    /** True while a call to the daemon is out. */
    var busy by mutableStateOf(false)
    var error by mutableStateOf<String?>(null)
    /** What the owner answered, by flag name, in the order given. */
    var answers by mutableStateOf<Map<String, String>>(emptyMap())
        private set
    var land by mutableStateOf("no")
    var prove by mutableStateOf(true)
    var progress by mutableStateOf<List<String>>(emptyList())
        private set
    /** What the apply warned of, and what it noted; kept until the wizard is closed. */
    var applyWarnings by mutableStateOf<List<String>>(emptyList())
        private set
    var applyNotes by mutableStateOf<List<String>>(emptyList())
        private set
    /** The project the apply registered. */
    var appliedId by mutableStateOf<String?>(null)
        private set
    /** Set while "Use recommended for the rest" is answering one question after another. */
    var usingRecommended by mutableStateOf(false)
        private set

    /** The `answers` the daemon reads: the owner's, the landing choice, and the proof task off when not wanted. */
    fun flags(): Map<String, Any?> {
        val m = linkedMapOf<String, Any?>()
        answers.forEach { (k, v) -> m[k] = v }
        m["land"] = land
        if (!prove) m["no_prove"] = true
        return m
    }

    fun planBody(): String = EditorJson.text(mapOf("path" to path.trim(), "answers" to flags()))

    fun answer(q: WizardQuestion, value: String) {
        answers = answers + (q.key to value)
    }

    /** Answer the open question with its recommended value and carry on with the rest the same way. */
    fun useRecommended() {
        val q = plan?.next ?: return
        usingRecommended = true
        answer(q, q.default)
    }

    fun candidatesArrived(json: String) {
        val o = EditorJson.parse(json)
        val a = o.optJSONArray("candidates")
        candidates = a?.let { arr ->
            (0 until arr.length()).map { i ->
                arr.getJSONObject(i).let { c ->
                    Candidate(c.optString("path"), c.optString("name"), c.optBoolean("has_mason"), if (c.isNull("remote")) null else c.optString("remote"))
                }
            }
        } ?: emptyList()
        val e = o.optJSONArray("errors")
        candidateErrors = e?.let { arr ->
            (0 until arr.length()).map { i -> arr.getJSONObject(i).let { "${it.optString("path")}: ${it.optString("error")}" } }
        } ?: emptyList()
    }

    /**
     * A plan arrived. Returns true when the wizard should ask for another plan (recommended
     * answers still being filled in).
     */
    fun planArrived(json: String): Boolean {
        busy = false
        val p = WizardPlan.parse(json)
        if (p == null) { error = "The daemon sent a plan this monitor could not read."; usingRecommended = false; return false }
        plan = p
        if (p.errors.isNotEmpty()) { error = p.errors.joinToString("\n"); usingRecommended = false; return false }
        error = null
        if (usingRecommended) {
            val q = p.next
            if (q != null && q.key !in answers) { answer(q, q.default); return true }
            usingRecommended = false
        }
        step = when {
            step == AddProjectStep.REPOSITORY -> AddProjectStep.DETECTED
            p.next == null -> AddProjectStep.REVIEW
            else -> AddProjectStep.QUESTIONS
        }
        return false
    }

    fun planFailed(why: String) { busy = false; usingRecommended = false; error = why }

    /** From Detected: on to the questions, or straight to the review when none are left. */
    fun continueFromDetected() {
        step = if (plan?.next == null) AddProjectStep.REVIEW else AddProjectStep.QUESTIONS
    }

    fun startApply() {
        step = AddProjectStep.APPLY
        busy = true
        error = null
        progress = emptyList()
        applyWarnings = emptyList()
        applyNotes = emptyList()
        appliedId = null
    }

    /** The apply's answer: the project's id when it was registered, else null with [error] set. */
    fun applyArrived(ok: Boolean, json: String, why: String): String? {
        busy = false
        val o = EditorJson.parse(json)
        val lines = EditorJson.strings(o.optJSONArray("progress"))
        progress = lines
        applyWarnings = EditorJson.strings(o.optJSONArray("warnings"))
        applyNotes = EditorJson.strings(o.optJSONArray("notes"))
        if (!ok) { error = why.ifEmpty { "The daemon refused." }; return null }
        val err = o.optString("error").takeIf { it.isNotEmpty() && !o.isNull("error") }
        val id = o.optString("id").takeIf { it.isNotEmpty() }
        if (err != null || id == null || !o.optBoolean("registered", true)) {
            error = err ?: "The project was written but not registered."
            return null
        }
        error = null
        appliedId = id
        return id
    }

    /** After a refused apply: back to the review with the answers kept. */
    fun reviewAgain() { step = AddProjectStep.REVIEW; error = null }

    /** One step back. Leaving the questions forgets the answers, so the daemon asks them afresh. */
    fun back(): Boolean {
        error = null
        usingRecommended = false
        when (step) {
            AddProjectStep.REPOSITORY, AddProjectStep.APPLY -> return false
            AddProjectStep.DETECTED -> { step = AddProjectStep.REPOSITORY; answers = emptyMap(); plan = null }
            AddProjectStep.QUESTIONS -> { step = AddProjectStep.DETECTED; answers = emptyMap() }
            AddProjectStep.REVIEW -> {
                if (answers.isEmpty()) { step = AddProjectStep.DETECTED; return false }
                answers = answers.entries.toList().dropLast(1).associate { it.key to it.value }
                step = AddProjectStep.QUESTIONS
                return true // the plan on show is for the old answers: ask again
            }
        }
        return false
    }
}
