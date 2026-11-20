package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * One revision-checked member lifecycle change.
 *
 * The revision comes from the member directory, so a stale operator cannot
 * overwrite a newer decision: the Coordinator rejects the CAS instead of
 * applying it. Actions are a closed set; unknown strings never reach the wire.
 */
data class MemberLifecycleRequest(
    val networkId: String,
    val targetNodeId: String,
    val action: String,
    val role: String = "member",
    val expectedRevision: Long,
    val reason: String? = null,
)

data class MemberChangeView(
    val networkId: String?,
    val targetNodeId: String?,
    val action: String?,
    val status: String?,
    val revision: Long?,
    val accessEpoch: Long?,
    val idempotent: Boolean,
    val accessRevoked: Boolean,
    val requiresReauthentication: Boolean,
    val requiresReapproval: Boolean,
)

object MemberLifecycleSurface {
    val ACTIONS = setOf(
        "invite",
        "approve",
        "reject",
        "suspend",
        "resume",
        "exit",
        "remove",
        "undo_remove",
    )

    val ROLES = setOf("owner", "administrator", "member")

    /**
     * Encodes the command. Returns null for an action or role this client does
     * not model, so a UI bug cannot turn into a protocol error.
     */
    fun command(request: MemberLifecycleRequest): JSONObject? {
        if (request.action !in ACTIONS) return null
        if (request.role !in ROLES) return null
        if (request.networkId.isBlank() || request.targetNodeId.isBlank()) return null
        if (request.expectedRevision < 0) return null
        val payload = JSONObject()
            .put("network_id", request.networkId)
            .put("target_node_id", request.targetNodeId)
            .put("action", request.action)
            .put("role", request.role)
            .put("expected_revision", request.expectedRevision)
        request.reason?.takeIf { it.isNotBlank() }?.let { payload.put("reason", it) }
        return JSONObject()
            .put("command", "change_member_lifecycle")
            .put("payload", payload)
    }

    fun parseChange(data: JSONObject?): MemberChangeView? {
        if (data == null) return null
        return MemberChangeView(
            networkId = data.stringOrNull("network_id"),
            targetNodeId = data.stringOrNull("target_node_id"),
            action = data.stringOrNull("action"),
            status = data.stringOrNull("status"),
            revision = data.longOrNull("revision"),
            accessEpoch = data.longOrNull("access_epoch"),
            idempotent = data.optBoolean("idempotent", false),
            accessRevoked = data.optBoolean("access_revoked", false),
            requiresReauthentication = data.optBoolean("requires_reauthentication", false),
            requiresReapproval = data.optBoolean("requires_reapproval", false),
        )
    }
}
