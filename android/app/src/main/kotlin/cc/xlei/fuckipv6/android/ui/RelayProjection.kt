package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCommandKind
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkRole
import cc.xlei.fuckipv6.contract.NodeContractV1.NodePermission
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSetNetworkRoleCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeUiAvailability
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayGovernanceAuditSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayGovernanceEvent
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthSnapshot

data class RelayActionDecision(
    val canSubmit: Boolean,
    val reasonKey: String?,
    val commandKind: NodeCommandKind,
    val permission: NodePermission,
    val availability: NodeUiAvailability,
) {
    fun command(networkName: String, enabled: Boolean): NodeSetNetworkRoleCommand {
        check(canSubmit) { "Relay action is blocked: $reasonKey" }
        return RelayActionPolicy.command(networkName, enabled)
    }
}

object RelayActionPolicy {
    val commandKind: NodeCommandKind = NodeCommandKind.SET_NETWORK_ROLE
    val permission: NodePermission = NodePermission.MANAGE_ROLES
    val availability: NodeUiAvailability = NodeUiAvailability.NETWORK_SELECTED

    fun evaluate(
        backendAvailable: Boolean,
        grantedPermissions: Set<NodePermission>,
        selectedNetwork: String?,
        requestedNetwork: String?,
    ): RelayActionDecision {
        val reasonKey = when {
            !backendAvailable -> "state.control_unavailable.detail"
            permission !in grantedPermissions -> "state.permission_denied.detail"
            selectedNetwork.isNullOrBlank() -> "choice.availability.network_selected"
            requestedNetwork.isNullOrBlank() || requestedNetwork != selectedNetwork ->
                "choice.availability.network_selected"
            else -> null
        }
        return RelayActionDecision(
            canSubmit = reasonKey == null,
            reasonKey = reasonKey,
            commandKind = commandKind,
            permission = permission,
            availability = availability,
        )
    }

    fun command(networkName: String, enabled: Boolean): NodeSetNetworkRoleCommand {
        require(networkName.isNotBlank()) { "networkName must not be blank" }
        return NodeSetNetworkRoleCommand(enabled, networkName, NodeNetworkRole.RELAY)
    }
}

data class RelayGovernanceProjection(
    val events: List<RelayGovernanceEvent>,
    val total: Int,
    val dropped: Long,
) {
    val shown: Int
        get() = events.size

    companion object {
        const val MAX_EVENTS = 50

        fun from(audit: RelayGovernanceAuditSnapshot): RelayGovernanceProjection {
            val allEvents = audit.events ?: emptyList()
            return RelayGovernanceProjection(
                events = allEvents.takeLast(MAX_EVENTS),
                total = allEvents.size,
                dropped = audit.dropped_events ?: 0L,
            )
        }
    }
}

data class RelayHealthProjection(
    val statusKey: String,
    val lifecycleKey: String,
    val reachabilityKey: String,
    val listenerBound: Boolean,
    val listenAddr: String?,
    val forwardingObserved: Boolean,
    val authorizationValid: Boolean,
    val quotaLimited: Boolean,
    val observedAtUnixMs: Long?,
    val reasonCode: String?,
    val issueKeys: List<String>,
) {
    companion object {
        fun from(health: RelayHealthSnapshot): RelayHealthProjection {
            requireNotNull(health.status) { "Relay health status is required" }
            requireNotNull(health.lifecycle) { "Relay lifecycle is required" }
            return RelayHealthProjection(
                statusKey = "relay.health.${health.status.wireName()}",
                lifecycleKey = "relay.lifecycle.${health.lifecycle.wireName()}",
                reachabilityKey = "relay.reachability.${health.network_reachability?.wireName() ?: "unknown"}",
                listenerBound = health.listener_bound == true,
                listenAddr = health.listen_addr,
                forwardingObserved = health.forwarding_observed == true,
                authorizationValid = health.authorization_valid == true,
                quotaLimited = health.quota_limited == true,
                observedAtUnixMs = health.observed_at_unix_ms,
                reasonCode = health.reason_code,
                issueKeys = health.issues.orEmpty().map { "relay.issue.${it.wireName()}" },
            )
        }
    }
}
