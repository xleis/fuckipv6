package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONObject

/**
 * Administrator-recovery request as the node publishes it in
 * `admin_recoveries` command results, reduced to the fields the Android
 * surface renders.
 *
 * Nothing is inferred: an absent field stays absent, an unreadable `state`
 * stays null (the page renders it as unknown), and `can_execute` is only true
 * when the node itself said so.
 */
data class RecoveryStatusView(
    val requestId: String?,
    val networkId: String?,
    val state: String?,
    val canExecute: Boolean,
    val denialKey: String?,
    val initiator: String?,
    val participants: List<String>?,
    val signatureCount: Long?,
    val requiredSignatures: Long?,
    val issuedAtUnixMs: Long?,
    val expiresAtUnixMs: Long?,
    val impact: RecoveryImpactView?,
    val revocation: RecoveryRevocationView?,
)

data class RecoveryImpactView(
    val administratorsAdded: List<String>?,
    val administratorsRemoved: List<String>?,
    val membersAdded: List<String>?,
    val membersRemoved: List<String>?,
    val ownerAfter: String?,
    val ownerBefore: String?,
)

data class RecoveryRevocationView(
    val oldSessions: Boolean?,
    val pendingInvitations: Boolean?,
    val unconfirmedWrites: Boolean?,
)

object AdminRecoverySurface {
    fun listCommand(networkName: String): JSONObject = JSONObject()
        .put("command", "list_admin_recoveries")
        .put("payload", JSONObject().put("network_name", networkName))

    /**
     * Parses the `admin_recoveries` outcome. The node returns an array; the
     * bridge also accepts a wrapped `items` array so a future envelope change
     * cannot silently turn a list into "no recoveries".
     */
    fun parseList(data: JSONObject?): List<RecoveryStatusView> {
        if (data == null) return emptyList()
        val array = when {
            data.has("items") && data.opt("items") is JSONArray -> data.optJSONArray("items")
            else -> {
                // A bare array arrives wrapped as {"items": [...]} by the parser.
                data.optJSONArray("items")
            }
        } ?: return emptyList()
        return (0 until array.length()).mapNotNull { index ->
            array.optJSONObject(index)?.let(::status)
        }
    }

    private fun status(json: JSONObject): RecoveryStatusView = RecoveryStatusView(
        requestId = json.stringOrNull("request_id"),
        networkId = json.stringOrNull("network_id"),
        state = json.stringOrNull("state"),
        canExecute = json.optBoolean("can_execute", false),
        denialKey = json.stringOrNull("denial_key"),
        initiator = json.stringOrNull("initiator"),
        participants = json.stringArray("participants"),
        signatureCount = json.numberOrNull("signature_count"),
        requiredSignatures = json.numberOrNull("required_signatures"),
        issuedAtUnixMs = json.numberOrNull("issued_at_unix_ms"),
        expiresAtUnixMs = json.numberOrNull("expires_at_unix_ms"),
        impact = json.optJSONObject("impact")?.let(::impact),
        revocation = json.optJSONObject("revocation")?.let(::revocation),
    )

    private fun impact(json: JSONObject) = RecoveryImpactView(
        administratorsAdded = json.stringArray("administrators_added"),
        administratorsRemoved = json.stringArray("administrators_removed"),
        membersAdded = json.stringArray("members_added"),
        membersRemoved = json.stringArray("members_removed"),
        ownerAfter = json.stringOrNull("owner_after"),
        ownerBefore = json.stringOrNull("owner_before"),
    )

    private fun revocation(json: JSONObject) = RecoveryRevocationView(
        oldSessions = json.booleanOrNull("old_sessions"),
        pendingInvitations = json.booleanOrNull("pending_invitations"),
        unconfirmedWrites = json.booleanOrNull("unconfirmed_writes"),
    )

    private fun JSONObject.stringArray(key: String): List<String>? {
        val array = optJSONArray(key) ?: return null
        return (0 until array.length()).mapNotNull { index ->
            array.optString(index, "").takeIf { it.isNotEmpty() }
        }
    }

    private fun JSONObject.numberOrNull(key: String): Long? {
        if (!has(key) || isNull(key)) return null
        return when (val value = opt(key)) {
            is Number -> value.toLong()
            is String -> value.toLongOrNull()
            else -> null
        }
    }

    private fun JSONObject.booleanOrNull(key: String): Boolean? =
        if (!has(key) || isNull(key)) null else optBoolean(key)
}