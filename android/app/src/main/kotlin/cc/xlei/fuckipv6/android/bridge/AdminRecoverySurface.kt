package cc.xlei.fuckipv6.android.bridge

import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryRequest
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

    fun statusCommand(networkName: String, requestId: String): JSONObject = JSONObject()
        .put("command", "get_admin_recovery_status")
        .put(
            "payload",
            JSONObject()
                .put("network_name", networkName)
                .put("request_id", requestId),
        )

    fun revokeCommand(networkName: String, requestId: String, reason: String): JSONObject = JSONObject()
        .put("command", "revoke_admin_recovery")
        .put(
            "payload",
            JSONObject()
                .put("network_name", networkName)
                .put("request_id", requestId)
                .put("reason", reason),
        )

    fun executeCommand(networkName: String, requestId: String): JSONObject = JSONObject()
        .put("command", "execute_admin_recovery")
        .put(
            "payload",
            JSONObject()
                .put("network_name", networkName)
                .put("request_id", requestId),
        )

    /**
     * Submits the exact signed bundle the operator assembled offline. The
     * request id, snapshot digest and approvals are carried verbatim so the
     * coordinator re-validates the same bytes it would see from a desktop.
     */
    fun submitCommand(networkName: String, request: AdminRecoveryRequest): JSONObject = JSONObject()
        .put("command", "submit_admin_recovery")
        .put(
            "payload",
            JSONObject()
                .put("network_name", networkName)
                .put("request", encodeRequest(request)),
        )

    fun encodeRequest(request: AdminRecoveryRequest): JSONObject {
        val proposal = request.proposal
        val snapshot = proposal.snapshot
        val target = proposal.target
        val revocation = proposal.revocation
        val approvals = JSONArray()
        request.approvals.orEmpty().forEach { approval ->
            approvals.put(
                JSONObject()
                    .put("signer", approval.signer)
                    .put("signer_public_key", approval.signer_public_key)
                    .put("signed_at_unix_ms", approval.signed_at_unix_ms)
                    .put("signature", approval.signature),
            )
        }
        return JSONObject()
            .put(
                "proposal",
                JSONObject()
                    .put("schema_version", proposal.schema_version)
                    .put("request_id", proposal.request_id)
                    .put("network_id", proposal.network_id)
                    .put(
                        "snapshot",
                        JSONObject()
                            .put("owner", snapshot.owner)
                            .put("administrators", JSONArray(snapshot.administrators.orEmpty()))
                            .put("members", JSONArray(snapshot.members.orEmpty()))
                            .put("network_revision", snapshot.network_revision)
                            .put("roster_digest", snapshot.roster_digest),
                    )
                    .put(
                        "target",
                        JSONObject()
                            .put("owner", target.owner)
                            .put("administrators", JSONArray(target.administrators.orEmpty()))
                            .put("members", JSONArray(target.members.orEmpty())),
                    )
                    .put("initiated_by", proposal.initiated_by)
                    .put("reason", proposal.reason.wireName())
                    .put("nonce", proposal.nonce)
                    .put("issued_at_unix_ms", proposal.issued_at_unix_ms)
                    .put("expires_at_unix_ms", proposal.expires_at_unix_ms)
                    .put(
                        "revocation",
                        JSONObject()
                            .put("old_sessions", revocation.old_sessions)
                            .put("pending_invitations", revocation.pending_invitations)
                            .put("unconfirmed_writes", revocation.unconfirmed_writes),
                    ),
            )
            .put("approvals", approvals)
    }

    /** Single status payload returned by submit/status/revoke/execute. */
    fun parseSingle(data: JSONObject?): RecoveryStatusView? {
        if (data == null) return null
        val source = if (data.has("request_id")) data else data.optJSONObject("status")
        return source?.let(::status)
    }

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