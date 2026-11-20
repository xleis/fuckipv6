package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.RecoveryImpactView
import cc.xlei.fuckipv6.android.bridge.RecoveryRevocationView
import cc.xlei.fuckipv6.android.bridge.RecoveryStatusView
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryApproval
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryImpact
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryProposal
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryReason
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryRequest
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryRevocationPlan
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoverySnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryState
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryStatusView
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryTarget
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryListCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryStatusCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeExecuteAdminRecoveryCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeRevokeAdminRecoveryCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSubmitAdminRecoveryCommand
import org.json.JSONArray
import org.json.JSONObject
import java.util.Base64

data class AdminRecoveryImpactProjection(
    val administratorsAdded: List<String>?,
    val administratorsRemoved: List<String>?,
    val membersAdded: List<String>?,
    val membersRemoved: List<String>?,
    val ownerAfter: String?,
    val ownerBefore: String?,
) {
    companion object {
        fun from(impact: AdminRecoveryImpact): AdminRecoveryImpactProjection =
            AdminRecoveryImpactProjection(
                administratorsAdded = impact.administrators_added,
                administratorsRemoved = impact.administrators_removed,
                membersAdded = impact.members_added,
                membersRemoved = impact.members_removed,
                ownerAfter = impact.owner_after,
                ownerBefore = impact.owner_before,
            )
    }
}

data class AdminRecoveryRevocationPlanProjection(
    val oldSessions: Boolean?,
    val pendingInvitations: Boolean?,
    val unconfirmedWrites: Boolean?,
) {
    companion object {
        fun from(plan: AdminRecoveryRevocationPlan): AdminRecoveryRevocationPlanProjection =
            AdminRecoveryRevocationPlanProjection(
                oldSessions = plan.old_sessions,
                pendingInvitations = plan.pending_invitations,
                unconfirmedWrites = plan.unconfirmed_writes,
            )
    }
}

/** Safe enum lookup: the generated `fromWireName` throws on unknown values. */
internal fun adminRecoveryState(value: String?): AdminRecoveryState? = value?.let {
    runCatching { AdminRecoveryState.fromWireName(it) }.getOrNull()
}

data class AdminRecoveryStatusProjection(
    val canExecute: Boolean?,
    val denialKey: String?,
    val expiresAtUnixMs: Long?,
    val impact: AdminRecoveryImpactProjection?,
    val initiator: String?,
    val issuedAtUnixMs: Long?,
    val networkId: String?,
    val participants: List<String>?,
    val requestId: String?,
    val requiredSignatures: Long?,
    val revocation: AdminRecoveryRevocationPlanProjection?,
    val signatureCount: Long?,
    val state: AdminRecoveryState?,
) {
    companion object {
        fun from(status: AdminRecoveryStatusView): AdminRecoveryStatusProjection =
            AdminRecoveryStatusProjection(
                canExecute = status.can_execute,
                denialKey = status.denial_key,
                expiresAtUnixMs = status.expires_at_unix_ms,
                impact = status.impact?.let(AdminRecoveryImpactProjection::from),
                initiator = status.initiator,
                issuedAtUnixMs = status.issued_at_unix_ms,
                networkId = status.network_id,
                participants = status.participants,
                requestId = status.request_id,
                requiredSignatures = status.required_signatures,
                revocation = status.revocation?.let(
                    AdminRecoveryRevocationPlanProjection::from,
                ),
                signatureCount = status.signature_count,
                state = status.state,
            )

        /** Bridge entry point: same projection from the JSON read model. */
        fun from(view: RecoveryStatusView): AdminRecoveryStatusProjection =
            AdminRecoveryStatusProjection(
                canExecute = view.canExecute,
                denialKey = view.denialKey,
                expiresAtUnixMs = view.expiresAtUnixMs,
                impact = view.impact?.let { impact: RecoveryImpactView ->
                    AdminRecoveryImpactProjection(
                        administratorsAdded = impact.administratorsAdded,
                        administratorsRemoved = impact.administratorsRemoved,
                        membersAdded = impact.membersAdded,
                        membersRemoved = impact.membersRemoved,
                        ownerAfter = impact.ownerAfter,
                        ownerBefore = impact.ownerBefore,
                    )
                },
                initiator = view.initiator,
                issuedAtUnixMs = view.issuedAtUnixMs,
                networkId = view.networkId,
                participants = view.participants,
                requestId = view.requestId,
                requiredSignatures = view.requiredSignatures,
                revocation = view.revocation?.let { revocation: RecoveryRevocationView ->
                    AdminRecoveryRevocationPlanProjection(
                        oldSessions = revocation.oldSessions,
                        pendingInvitations = revocation.pendingInvitations,
                        unconfirmedWrites = revocation.unconfirmedWrites,
                    )
                },
                signatureCount = view.signatureCount,
                state = adminRecoveryState(view.state),
            )
    }
}

data class AdminRecoveryPageData(
    val networkName: String,
    val networkId: String?,
    val backendAvailable: Boolean,
    val latestStatus: AdminRecoveryStatusProjection? = null,
    val recoveries: List<AdminRecoveryStatusProjection> = emptyList(),
    val loadedAtUnixMs: Long? = null,
    val refreshedAtUnixMs: Long? = null,
    val statusRefreshSequence: Long = 0,
    val onSubmit: ((NodeSubmitAdminRecoveryCommand) -> Unit)? = null,
    val onGetStatus: ((NodeAdminRecoveryStatusCommand) -> Unit)? = null,
    val onList: ((NodeAdminRecoveryListCommand) -> Unit)? = null,
    val onRevoke: ((NodeRevokeAdminRecoveryCommand) -> Unit)? = null,
    val onExecute: ((NodeExecuteAdminRecoveryCommand) -> Unit)? = null,
) {
    fun recordStatus(
        status: AdminRecoveryStatusProjection,
        refreshedAtUnixMs: Long,
    ): AdminRecoveryPageData {
        val requestId = status.requestId
        val updatedRecoveries = if (requestId == null) {
            recoveries
        } else {
            listOf(status) + recoveries.filterNot { it.requestId == requestId }
        }
        return copy(
            latestStatus = status,
            recoveries = updatedRecoveries,
            refreshedAtUnixMs = refreshedAtUnixMs,
            statusRefreshSequence = statusRefreshSequence + 1,
        )
    }

    fun recordRecoveries(
        items: List<AdminRecoveryStatusProjection>,
        loadedAtUnixMs: Long,
    ): AdminRecoveryPageData = copy(
        recoveries = items,
        loadedAtUnixMs = loadedAtUnixMs,
    )

    companion object {
        fun fromSnapshot(
            snapshot: NodeSnapshot,
            selectedNetworkName: String,
            backendAvailable: Boolean,
            latestStatus: AdminRecoveryStatusProjection? = null,
            recoveries: List<AdminRecoveryStatusProjection> = emptyList(),
            loadedAtUnixMs: Long? = null,
            refreshedAtUnixMs: Long? = null,
            statusRefreshSequence: Long = 0,
            onSubmit: ((NodeSubmitAdminRecoveryCommand) -> Unit)? = null,
            onGetStatus: ((NodeAdminRecoveryStatusCommand) -> Unit)? = null,
            onList: ((NodeAdminRecoveryListCommand) -> Unit)? = null,
            onRevoke: ((NodeRevokeAdminRecoveryCommand) -> Unit)? = null,
            onExecute: ((NodeExecuteAdminRecoveryCommand) -> Unit)? = null,
        ): AdminRecoveryPageData? {
            if (selectedNetworkName.isBlank()) return null
            val membership = snapshot.memberships.orEmpty().firstOrNull {
                it.network_name == selectedNetworkName
            } ?: return null
            return AdminRecoveryPageData(
                networkName = selectedNetworkName,
                networkId = membership.network_id,
                backendAvailable = backendAvailable,
                latestStatus = latestStatus,
                recoveries = recoveries,
                loadedAtUnixMs = loadedAtUnixMs,
                refreshedAtUnixMs = refreshedAtUnixMs,
                statusRefreshSequence = statusRefreshSequence,
                onSubmit = onSubmit,
                onGetStatus = onGetStatus,
                onList = onList,
                onRevoke = onRevoke,
                onExecute = onExecute,
            )
        }

        /**
         * Bridge entry point: builds the page from the JSON read model. Only the
         * refresh action is wired, because submitting, revoking and executing
         * need the full signed request bundle that the desktop surfaces own.
         * Unwired actions stay disabled rather than pretending success.
         */
        fun fromBridge(
            networkName: String,
            networkId: String?,
            recoveries: List<RecoveryStatusView>,
            loadedAtUnixMs: Long? = null,
            onList: ((NodeAdminRecoveryListCommand) -> Unit)? = null,
        ): AdminRecoveryPageData = AdminRecoveryPageData(
            networkName = networkName,
            networkId = networkId,
            backendAvailable = onList != null,
            latestStatus = recoveries.firstOrNull()?.let(AdminRecoveryStatusProjection::from),
            recoveries = recoveries.map(AdminRecoveryStatusProjection::from),
            loadedAtUnixMs = loadedAtUnixMs,
            onList = onList,
        )
    }
}

object AdminRecoveryRequestCodec {
    fun parse(raw: String): AdminRecoveryRequest {
        val root = JSONObject(raw)
        val proposal = root.getJSONObject("proposal")
        val requestId = proposal.requiredString("request_id")
        require(validRequestId(requestId)) { "invalid admin recovery request id" }
        val revocation = optionalObject(proposal, "revocation")?.let(::parseRevocation)
            ?: AdminRecoveryRevocationPlan(true, true, true)
        val snapshot = proposal.getJSONObject("snapshot").let { value ->
            AdminRecoverySnapshot(
                requiredStringList(value, "administrators"),
                requiredStringList(value, "members"),
                value.getLong("network_revision"),
                value.requiredString("owner"),
                value.requiredString("roster_digest"),
            )
        }
        val target = proposal.getJSONObject("target").let { value ->
            AdminRecoveryTarget(
                optionalStringList(value, "administrators") ?: emptyList(),
                requiredStringList(value, "members"),
                value.requiredString("owner"),
            )
        }
        val parsedProposal = AdminRecoveryProposal(
            proposal.getLong("expires_at_unix_ms"),
            proposal.requiredString("initiated_by"),
            proposal.getLong("issued_at_unix_ms"),
            proposal.requiredString("network_id"),
            proposal.requiredString("nonce"),
            AdminRecoveryReason.fromWireName(proposal.requiredString("reason")),
            requestId,
            revocation,
            proposal.getInt("schema_version"),
            snapshot,
            target,
        )
        val approvals = optionalArray(root, "approvals")?.let { values ->
            buildList {
                for (index in 0 until values.length()) {
                    val approval = values.getJSONObject(index)
                    add(
                        AdminRecoveryApproval(
                            approval.requiredString("signature"),
                            approval.getLong("signed_at_unix_ms"),
                            approval.requiredString("signer"),
                            approval.requiredString("signer_public_key"),
                        ),
                    )
                }
            }
        }.orEmpty()
        return AdminRecoveryRequest(approvals, parsedProposal)
    }

    fun validRequestId(value: String): Boolean {
        if (!value.matches(Regex("^recovery_[A-Za-z0-9_-]{43}$"))) return false
        return runCatching {
            val encoded = value.removePrefix("recovery_")
            Base64.getUrlDecoder().decode(encoded).size == 32
        }.getOrDefault(false)
    }

    fun validRevocationReason(value: String): Boolean =
        value.trim() == value &&
            value.isNotEmpty() &&
            value.none { it.isISOControl() } &&
            value.codePointCount(0, value.length) <= 512

    private fun parseRevocation(value: JSONObject): AdminRecoveryRevocationPlan =
        AdminRecoveryRevocationPlan(
            value.getBoolean("old_sessions"),
            value.getBoolean("pending_invitations"),
            value.getBoolean("unconfirmed_writes"),
        )

    private fun JSONObject.requiredString(name: String): String =
        getString(name).also { require(it.isNotBlank()) { "$name must not be blank" } }

    private fun requiredStringList(value: JSONObject, name: String): List<String> =
        stringList(value.getJSONArray(name))

    private fun optionalStringList(value: JSONObject, name: String): List<String>? =
        optionalArray(value, name)?.let(::stringList)

    private fun stringList(values: JSONArray): List<String> = buildList {
        for (index in 0 until values.length()) {
            val item = values.getString(index)
            require(item.isNotBlank()) { "list item must not be blank" }
            add(item)
        }
    }

    private fun optionalArray(value: JSONObject, name: String): JSONArray? = when {
        !value.has(name) || value.isNull(name) -> null
        else -> value.getJSONArray(name)
    }

    private fun optionalObject(value: JSONObject, name: String): JSONObject? = when {
        !value.has(name) || value.isNull(name) -> null
        else -> value.getJSONObject(name)
    }
}
