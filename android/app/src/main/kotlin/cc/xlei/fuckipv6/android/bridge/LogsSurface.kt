package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * One node log line.
 *
 * The snapshot only carries logs for the node owner, and ownership is not a
 * field of the projected snapshot — it only decides whether the array is
 * populated. An empty list therefore means "no log lines were published in this
 * snapshot", which covers both "owner with nothing yet" and "not the owner".
 * The UI says exactly that instead of claiming the node is quiet.
 */
data class LogLineView(
    val level: String,
    val target: String,
    val message: String,
    val unixMs: Long?,
)

data class LogsFeed(
    val lines: List<LogLineView>,
) {
    val hasLines: Boolean
        get() = lines.isNotEmpty()
}

object LogsSurface {
    const val MAX_LINES = 200
    const val MAX_MESSAGE_CHARS = 500

    fun parse(snapshot: JSONObject): LogsFeed? {
        val raw = snapshot.opt("logs") ?: return null
        val array = when (raw) {
            is org.json.JSONArray -> raw
            JSONObject.NULL -> org.json.JSONArray()
            else -> return null
        }
        val lines = (0 until array.length()).mapNotNull { index ->
            val item = array.optJSONObject(index) ?: return@mapNotNull null
            line(item)
        }.takeLast(MAX_LINES)
        return LogsFeed(lines = lines)
    }

    private fun line(json: JSONObject): LogLineView =
        LogLineView(
            level = json.stringOrNull("level") ?: "unknown",
            target = json.stringOrNull("target") ?: "",
            message = (json.stringOrNull("message") ?: "").take(MAX_MESSAGE_CHARS),
            unixMs = json.longOrNull("unix_ms"),
        )
}
