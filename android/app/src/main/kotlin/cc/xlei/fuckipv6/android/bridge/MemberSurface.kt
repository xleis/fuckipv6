package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONObject

/**
 * Director of the network's members as the node publishes it.
 *
 * Only fields the Android surface may show are kept: identity, the optional
 * display name the owner chose to publish, role, lifecycle state and the
 * actions the viewer is actually allowed to take. Presence and capability
 * details stay out until a page renders them.
 */
data class MemberDirectoryView(
    val networkId: String,
    val viewerRole: String?,
    val canManageMembers: Boolean,
    val networkRevision: Long,
    val members: List<MemberView>,
)

data class MemberView(
    val nodeId: String,
    val displayName: String?,
    val role: String?,
    val status: String?,
    val lifecycleState: String?,
    val revision: Long?,
    val accessEpoch: Long?,
    val availableActions: List<String>,
    val joinedAtUnixMs: Long?,
)

object MemberSurface {
    fun listCommand(networkId: String): JSONObject = JSONObject()
        .put("command", "list_network_members")
        .put("payload", JSONObject().put("network_id", networkId))

    /**
     * Parses the `network_members` outcome. A directory without members is a
     * valid empty network; a payload that is not a directory at all is `null`
     * so the caller keeps showing "unobserved" instead of an empty roster.
     */
    fun parseDirectory(data: JSONObject?): MemberDirectoryView? {
        if (data == null) return null
        val networkId = data.stringOrNull("network_id") ?: return null
        val members = data.opt("members")?.let { raw ->
            when (raw) {
                null, JSONObject.NULL -> emptyList()
                is JSONArray -> (0 until raw.length()).mapNotNull { index ->
                    raw.optJSONObject(index)?.let(::member)
                }
                else -> return null
            }
        }.orEmpty()
        return MemberDirectoryView(
            networkId = networkId,
            viewerRole = data.stringOrNull("viewer_role"),
            canManageMembers = data.optBoolean("can_manage_members", false),
            networkRevision = data.longOrNull("network_revision") ?: 0L,
            members = members,
        )
    }

    private fun member(json: JSONObject): MemberView? {
        val nodeId = json.stringOrNull("node_id") ?: return null
        return MemberView(
            nodeId = nodeId,
            displayName = json.stringOrNull("display_name"),
            role = json.stringOrNull("role"),
            status = json.stringOrNull("status"),
            lifecycleState = json.stringOrNull("lifecycle_state"),
            revision = json.longOrNull("revision"),
            accessEpoch = json.longOrNull("access_epoch"),
            availableActions = json.optJSONArray("available_actions")?.let { actions ->
                (0 until actions.length()).mapNotNull { index ->
                    actions.optString(index, "").takeIf { it.isNotEmpty() }
                }
            }.orEmpty(),
            joinedAtUnixMs = json.longOrNull("joined_at_unix_ms"),
        )
    }
}