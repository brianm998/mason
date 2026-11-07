// SPDX-License-Identifier: GPL-3.0-or-later
package mason.monitor

import org.json.JSONArray
import org.json.JSONObject

/** The small JSON the pipeline editor sends and reads (operations out, schemas and values in). */
object EditorJson {
    private fun wrap(v: Any?): Any? = when (v) {
        null -> JSONObject.NULL
        is Map<*, *> -> JSONObject().also { o -> v.forEach { (k, x) -> o.put(k.toString(), wrap(x)) } }
        is Iterable<*> -> JSONArray().also { a -> v.forEach { a.put(wrap(it)) } }
        else -> v
    }

    fun text(op: Map<String, Any?>): String = (wrap(op) as JSONObject).toString()

    fun parse(text: String): JSONObject = try { JSONObject(text) } catch (e: Exception) { JSONObject() }

    fun strings(a: JSONArray?): List<String> = a?.let { (0 until it.length()).map { i -> it.getString(i) } } ?: emptyList()

    /** A step form's fields, in the order the schema's `x-order` gives. */
    class Field(val key: String, val title: String, val type: String, val choices: List<String>, val description: String)

    fun fields(schema: String): List<Field> {
        val s = parse(schema)
        val props = s.optJSONObject("properties") ?: return emptyList()
        val order = strings(s.optJSONArray("x-order")).ifEmpty { props.keys().asSequence().toList().sorted() }
        return order.mapNotNull { k ->
            val p = props.optJSONObject(k) ?: return@mapNotNull null
            val choices = strings(p.optJSONArray("enum")).ifEmpty { strings(p.optJSONObject("items")?.optJSONArray("enum")) }
            Field(k, p.optString("title", k), p.optString("type", "string"), choices, p.optString("description", ""))
        }
    }
}
