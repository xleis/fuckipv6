package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONObject

/** One invitation as the node lists it. The code itself is never re-readable. */
data class InvitationView(
    val invitationId: String,
    val role: String?,
    val state: String?,
    val maxUses: Long?,
    val usedCount: Long?,
    val remainingUses: Long?,
    val revision: Long?,
    val createdAtUnixMs: Long?,
    val expiresAtUnixMs: Long?,
    val revokedAtUnixMs: Long?,
)

/** The single-read result of creating an invitation. */
data class IssuedInvitationView(
    val code: String,
    val invitation: InvitationView?,
)

data class CreateInvitationRequest(
    val networkId: String,
    val role: String = "member",
    val capabilities: List<String> = emptyList(),
    val maxUses: Int = 1,
    val expiresInMs: Long = DEFAULT_EXPIRY_MS,
) {
    companion object {
        /** Domain bound: one minute through thirty days. */
        const val MIN_EXPIRY_MS = 60_000L
        const val MAX_EXPIRY_MS = 30L * 24 * 60 * 60 * 1000
        const val DEFAULT_EXPIRY_MS = 24L * 60 * 60 * 1000
    }
}

object InvitationSurface {
    val ROLES = setOf("owner", "administrator", "member")

    fun listCommand(networkId: String): JSONObject? {
        if (networkId.isBlank()) return null
        return JSONObject()
            .put("command", "list_invitations")
            .put("payload", JSONObject().put("network_id", networkId))
    }

    /**
     * Encodes a creation request. Values the domain would reject (unknown role,
     * zero or negative uses, an out-of-range lifetime) are refused here instead
     * of being sent and bounced back as an opaque error.
     */
    fun createCommand(request: CreateInvitationRequest): JSONObject? {
        if (request.networkId.isBlank()) return null
        if (request.role !in ROLES) return null
        if (request.maxUses <= 0) return null
        if (request.expiresInMs !in CreateInvitationRequest.MIN_EXPIRY_MS..CreateInvitationRequest.MAX_EXPIRY_MS) {
            return null
        }
        val capabilities = JSONArray()
        request.capabilities.forEach { capabilities.put(it) }
        return JSONObject()
            .put("command", "create_invitation")
            .put(
                "payload",
                JSONObject()
                    .put("network_id", request.networkId)
                    .put("role", request.role)
                    .put("capabilities", capabilities)
                    .put("max_uses", request.maxUses)
                    .put("expires_in_ms", request.expiresInMs),
            )
    }

    fun revokeCommand(networkId: String, invitationId: String, expectedRevision: Long): JSONObject? {
        if (networkId.isBlank() || invitationId.isBlank()) return null
        if (expectedRevision <= 0) return null
        return JSONObject()
            .put("command", "revoke_invitation")
            .put(
                "payload",
                JSONObject()
                    .put("network_id", networkId)
                    .put("invitation_id", invitationId)
                    .put("expected_revision", expectedRevision),
            )
    }

    fun parseList(data: JSONObject?): List<InvitationView> {
        if (data == null) return emptyList()
        val array = data.optJSONArray("items") ?: return emptyList()
        return (0 until array.length()).mapNotNull { index ->
            array.optJSONObject(index)?.let(::status)
        }
    }

    fun parseIssued(data: JSONObject?): IssuedInvitationView? {
        if (data == null) return null
        val code = data.stringOrNull("code") ?: return null
        return IssuedInvitationView(
            code = code,
            invitation = data.optJSONObject("status")?.let(::status),
        )
    }

    private fun status(json: JSONObject): InvitationView? {
        val record = json.optJSONObject("invitation") ?: json
        val invitationId = record.stringOrNull("invitation_id") ?: return null
        return InvitationView(
            invitationId = invitationId,
            role = record.stringOrNull("role"),
            state = json.stringOrNull("state"),
            maxUses = record.longOrNull("max_uses"),
            usedCount = record.longOrNull("used_count"),
            remainingUses = json.longOrNull("remaining_uses"),
            revision = record.longOrNull("revision"),
            createdAtUnixMs = record.longOrNull("created_at_unix_ms"),
            expiresAtUnixMs = record.longOrNull("expires_at_unix_ms"),
            revokedAtUnixMs = record.longOrNull("revoked_at_unix_ms"),
        )
    }
}
