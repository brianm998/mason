// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import java.util.prefs.Preferences as JavaPrefs

/** Per-device memory: collapsed sidebar sections, text scale, mute. */
interface Preferences {
    fun bool(key: String): Boolean?
    fun setBool(key: String, value: Boolean)
    fun double(key: String): Double?
    fun setDouble(key: String, value: Double)
}

class MemoryPreferences : Preferences {
    private val values = mutableMapOf<String, Any>()
    override fun bool(key: String) = values[key] as? Boolean
    override fun setBool(key: String, value: Boolean) { values[key] = value }
    override fun double(key: String) = values[key] as? Double
    override fun setDouble(key: String, value: Double) { values[key] = value }
}

/** The user's own preferences store (the registry on Windows, plists on macOS, files on Linux). */
class SystemPreferences(node: String = "org/mason/monitor") : Preferences {
    private val p = JavaPrefs.userRoot().node(node)
    override fun bool(key: String) = p.get(key, null)?.toBooleanStrictOrNull()
    override fun setBool(key: String, value: Boolean) = p.put(key, value.toString())
    override fun double(key: String) = p.get(key, null)?.toDoubleOrNull()
    override fun setDouble(key: String, value: Double) = p.put(key, value.toString())
}
