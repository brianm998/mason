// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

/** `GET /lan`: is the LAN listener up, and the names it can be reached at (s433). */
data class LanStatus(val enabled: Boolean = false, val hosts: List<String> = emptyList(), val port: Int = 0) {
    companion object {
        const val HOW_TO_TURN_ON = "LAN access is off. Set `[lan] enabled = true` in daemon.toml, or start masond with --lan, then restart it."

        @Suppress("UNCHECKED_CAST")
        fun parse(json: String): LanStatus {
            val v = Codec.json(json) ?: return LanStatus()
            return LanStatus(
                enabled = v["enabled"] as? Boolean ?: false,
                hosts = (v["hosts"] as? List<Any?> ?: emptyList()).filterIsInstance<String>(),
                port = (v["port"] as? Number)?.toInt() ?: 0,
            )
        }
    }
}

data class PairedDevice(val id: Long, val name: String, val role: String, val createdAt: Long, val lastSeen: Long?, val revokedAt: Long?) {
    val isRevoked: Boolean get() = revokedAt != null
}

/** `GET /devices`: live devices (most recently seen first, never-seen last) and revoked ones apart (most recently revoked first). */
data class DeviceList(val active: List<PairedDevice> = emptyList(), val revoked: List<PairedDevice> = emptyList()) {
    companion object {
        @Suppress("UNCHECKED_CAST")
        fun parse(json: String): DeviceList {
            val v = Codec.json(json) ?: return DeviceList()
            val rows = (v["devices"] as? List<Map<String, Any?>> ?: emptyList()).mapNotNull {
                val id = (it["id"] as? Number)?.toLong() ?: return@mapNotNull null
                PairedDevice(
                    id, it["name"] as? String ?: "device $id", it["role"] as? String ?: "?",
                    (it["created_at"] as? Number)?.toLong() ?: 0, (it["last_seen"] as? Number)?.toLong(),
                    (it["revoked_at"] as? Number)?.toLong(),
                )
            }
            return DeviceList(
                active = rows.filter { !it.isRevoked }
                    .sortedWith(compareBy<PairedDevice> { it.lastSeen == null }.thenByDescending { it.lastSeen ?: 0 }.thenBy { it.id }),
                revoked = rows.filter { it.isRevoked }
                    .sortedWith(compareByDescending<PairedDevice> { it.revokedAt ?: 0 }.thenBy { it.id }),
            )
        }

        /** "never", "just now", "5 min ago", "3 h ago", "2 d ago". */
        fun lastSeenText(ms: Long?, nowMs: Long): String {
            if (ms == null) return "never"
            val s = if (nowMs > ms) (nowMs - ms) / 1000 else 0
            return when {
                s < 60 -> "just now"
                s < 3600 -> "${s / 60} min ago"
                s < 86400 -> "${s / 3600} h ago"
                else -> "${s / 86400} d ago"
            }
        }
    }
}

/** The answer to `POST /pair/codes`, with the moment it arrived so the countdown runs on an injected clock. */
data class PairOffer(val code: String, val role: String, val url: String, val hosts: List<String>, val expiresInS: Int, val issuedAtMs: Long) {
    sealed interface State {
        data class Valid(val secondsLeft: Int) : State
        object Expired : State
    }

    /** Whole seconds left, rounded up, while the code works; [State.Expired] from the deadline on. */
    fun state(nowMs: Long): State {
        val end = issuedAtMs + maxOf(0, expiresInS) * 1000L
        if (nowMs >= end) return State.Expired
        return State.Valid(((end - nowMs + 999) / 1000).toInt())
    }

    companion object {
        @Suppress("UNCHECKED_CAST")
        fun parse(json: String, nowMs: Long): PairOffer? {
            val v = Codec.json(json) ?: return null
            val code = v["code"] as? String ?: return null
            val url = v["url"] as? String ?: return null
            if (code.length != 8 || !code.all { it in '0'..'9' } || url.isEmpty()) return null
            return PairOffer(
                code, v["role"] as? String ?: "owner", url,
                (v["hosts"] as? List<Any?> ?: emptyList()).filterIsInstance<String>(),
                (v["expires_in_s"] as? Number)?.toInt() ?: 0, nowMs,
            )
        }

        /** "4:59". */
        fun clock(seconds: Int): String = "%d:%02d".format(seconds / 60, seconds % 60)
    }
}
