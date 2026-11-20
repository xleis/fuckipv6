package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * The node's applied routing summary for one network.
 *
 * Read-only: editing routing needs the typed `update_routing` form, so this page
 * shows the authorities the node already applied. `defaultAction` and
 * `effectiveTarget` are the tagged enums the node publishes (`kind`/`value`), so
 * the surface prints the server's own vocabulary instead of inventing labels.
 */
data class RoutingSummaryView(
    val networkName: String,
    val defaultActionKind: String?,
    val defaultActionValue: String?,
    val targetKind: String?,
    val targetValue: String?,
    val enabledRuleCount: Int,
    val lastAppliedRevision: Long?,
    val lastAppliedAtUnixMs: Long?,
    val status: String?,
    val problemCode: String?,
)

object RoutingSurface {
    fun parse(snapshot: JSONObject): List<RoutingSummaryView>? {
        val raw = snapshot.opt("routing_summaries") ?: return null
        val array = when (raw) {
            is org.json.JSONArray -> raw
            JSONObject.NULL -> org.json.JSONArray()
            else -> return null
        }
        return (0 until array.length()).mapNotNull { index ->
            array.optJSONObject(index)?.let(::summary)
        }
    }

    private fun summary(json: JSONObject): RoutingSummaryView? {
        val networkName = json.stringOrNull("network_name") ?: return null
        val action = json.optJSONObject("default_action")
        val target = json.optJSONObject("effective_target")
        return RoutingSummaryView(
            networkName = networkName,
            defaultActionKind = action?.stringOrNull("kind"),
            defaultActionValue = action?.stringOrNull("value"),
            targetKind = target?.stringOrNull("kind"),
            targetValue = target?.stringOrNull("value"),
            enabledRuleCount = json.longOrNull("enabled_rule_count")?.toInt() ?: 0,
            lastAppliedRevision = json.longOrNull("last_applied_revision"),
            lastAppliedAtUnixMs = json.longOrNull("last_applied_at_unix_ms"),
            status = json.stringOrNull("status"),
            problemCode = json.optJSONObject("problem")?.stringOrNull("code"),
        )
    }
}
