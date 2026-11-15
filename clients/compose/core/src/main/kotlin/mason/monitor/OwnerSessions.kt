// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import mason.v1.ActiveSession

/** The owner's chat with an agent (s364) as the dialog shows it. */
data class OwnerSessionTranscript(
    val id: Long = 0,
    val kind: String = "",
    val state: String = "",
    val working: Boolean = false,
    val messages: List<Message> = emptyList(),
) {
    data class Message(val id: Long, val role: String, val text: String)

    companion object {
        /** From the body of `GET /sessions/{id}`. */
        fun parse(json: String): OwnerSessionTranscript {
            val s = (Codec.json(json)?.get("session") as? Map<*, *>) ?: return OwnerSessionTranscript()
            return OwnerSessionTranscript(
                id = (s["id"] as? Number)?.toLong() ?: 0,
                kind = s["kind"] as? String ?: "",
                state = s["state"] as? String ?: "",
                working = s["working"] as? Boolean ?: false,
                messages = (s["messages"] as? List<*>)?.mapNotNull { it as? Map<*, *> }?.map {
                    Message((it["id"] as? Number)?.toLong() ?: 0, it["role"] as? String ?: "", it["text"] as? String ?: "")
                } ?: emptyList(),
            )
        }
    }
}

/** The sidebar's panel of open owner sessions (s432). */
object ActiveSessions {
    /** The panel's title; null when no session is open (the panel is hidden). */
    fun title(sessions: List<ActiveSession>): String? =
        if (sessions.isEmpty()) null else "Active sessions (${sessions.size})"

    /** The marker beside a session: replying, waiting for the owner, or none. */
    fun marker(s: ActiveSession): String = if (s.working) "replying" else if (s.waiting) "waiting for you" else ""
}
