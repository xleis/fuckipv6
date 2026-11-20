package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * One audit entry as the node publishes it.
 *
 * `summary` is server-classified copy, so the surface renders it verbatim and
 * never invents a title from the action name. Entries without an `audit_id`
 * are dropped: an activity row without stable identity cannot be merged or
 * trusted.
 */
data class ActivityEntryView(
    val auditId: String,
    val action: String,
    val resource: String,
    val outcome: String,
    val category: String?,
    val severity: String?,
    val occurredAtUnixMs: Long?,
    val summary: String?,
)

data class ActivityFeed(
    val networkId: String?,
    val entries: List<ActivityEntryView>,
)

object ActivitySurface {
    const val MAX_ENTRIES = 50

    /**
     * Recent activity for one network, newest first. `networkId == null` keeps
     * only network-agnostic entries; a network selection also keeps those
     * because the node records them outside any network.
     */
    fun parse(snapshot: JSONObject, networkId: String?): ActivityFeed? {
        val array = snapshot.opt("recent_activity")?.let { raw ->
            when (raw) {
                null, JSONObject.NULL -> org.json.JSONArray()
                is org.json.JSONArray -> raw
                else -> return null
            }
        } ?: return null
        val entries = (0 until array.length()).mapNotNull { index ->
            val item = array.optJSONObject(index) ?: return@mapNotNull null
            entry(item)
        }.filter { networkId == null || it.second.second == null || it.second.second == networkId }
            .sortedByDescending { it.first.occurredAtUnixMs ?: 0L }
            .take(MAX_ENTRIES)
            .map { it.first }
        return ActivityFeed(networkId = networkId, entries = entries)
    }

    private fun entry(json: JSONObject): Pair<ActivityEntryView, Pair<String, String?>>? {
        val auditId = json.stringOrNull("audit_id") ?: return null
        val action = json.stringOrNull("action") ?: return null
        val resource = json.stringOrNull("resource") ?: ""
        val networkId = json.stringOrNull("network_id")
        val view = ActivityEntryView(
            auditId = auditId,
            action = action,
            resource = resource,
            outcome = json.stringOrNull("outcome") ?: "unknown",
            category = json.stringOrNull("category"),
            severity = json.stringOrNull("severity"),
            occurredAtUnixMs = json.longOrNull("occurred_at_unix_ms"),
            summary = json.stringOrNull("summary"),
        )
        return view to (auditId to networkId)
    }
}
