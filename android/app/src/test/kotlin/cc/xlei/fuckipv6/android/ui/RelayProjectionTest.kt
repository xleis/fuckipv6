package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.contract.NodeContractV1.NodeAccessModel
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCommandKind
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeEmbeddedServiceSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeEmbeddedServiceStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkRole
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodePermission
import cc.xlei.fuckipv6.contract.NodeContractV1.NodePermissionSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeUiAvailability
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayGovernanceAction
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayGovernanceAuditSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayGovernanceEvent
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayLifecycleState
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayReachabilityState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class RelayProjectionTest {
    @Test
    fun relayActionUsesCanonicalCommandPermissionAndAvailability() {
        val allowed = RelayActionPolicy.evaluate(
            backendAvailable = true,
            grantedPermissions = setOf(NodePermission.MANAGE_ROLES),
            selectedNetwork = "home",
            requestedNetwork = "home",
        )

        assertTrue(allowed.canSubmit)
        assertNull(allowed.reasonKey)
        assertEquals(NodeCommandKind.SET_NETWORK_ROLE, allowed.commandKind)
        assertEquals(NodePermission.MANAGE_ROLES, allowed.permission)
        assertEquals(NodeUiAvailability.NETWORK_SELECTED, allowed.availability)
        val command = allowed.command("home", enabled = true)
        assertEquals("home", command.network_name)
        assertEquals(NodeNetworkRole.RELAY, command.role)
        assertEquals(true, command.enabled)
    }

    @Test
    fun relayActionFailsClosedWithoutBackendPermissionOrNetworkSelection() {
        val cases = listOf(
            Triple(false, setOf(NodePermission.MANAGE_ROLES), "home") to "state.control_unavailable.detail",
            Triple(true, emptySet<NodePermission>(), "home") to "state.permission_denied.detail",
            Triple(true, setOf(NodePermission.MANAGE_ROLES), null) to "choice.availability.network_selected",
        )

        cases.forEach { (input, expectedReason) ->
            val (backendAvailable, permissions, selectedNetwork) = input
            val decision = RelayActionPolicy.evaluate(
                backendAvailable = backendAvailable,
                grantedPermissions = permissions,
                selectedNetwork = selectedNetwork,
                requestedNetwork = "home",
            )
            assertFalse(decision.canSubmit)
            assertEquals(expectedReason, decision.reasonKey)
        }
        assertEquals(
            "choice.availability.network_selected",
            RelayActionPolicy.evaluate(
                backendAvailable = true,
                grantedPermissions = setOf(NodePermission.MANAGE_ROLES),
                selectedNetwork = "home",
                requestedNetwork = "other",
            ).reasonKey,
        )
    }

    @Test
    fun listenerProjectionReadsOnlyAuthoritativeHealthBindingAndAddress() {
        val health = relayHealth()

        val projection = RelayHealthProjection.from(health)

        assertFalse(projection.listenerBound)
        assertEquals("203.0.113.9:4433", projection.listenAddr)
        assertEquals("relay.health.unavailable", projection.statusKey)
        assertEquals("relay.lifecycle.running", projection.lifecycleKey)
        assertEquals("relay.reachability.unreachable", projection.reachabilityKey)
        assertEquals(123L, projection.observedAtUnixMs)
        assertEquals("listener_unavailable", projection.reasonCode)
        assertEquals(listOf("relay.issue.listener_unavailable"), projection.issueKeys)
    }

    @Test
    fun snapshotProjectionUsesAuthoritativeRoleAndSafeRelayFields() {
        val health = relayHealth()
        val governance = RelayGovernanceAuditSnapshot(
            2L,
            listOf(
                RelayGovernanceEvent(
                    RelayGovernanceAction.ENABLE,
                    null,
                    "node-safe",
                    null,
                    100L,
                    null,
                    null,
                    null,
                    null,
                ),
            ),
        )
        val data = RelayPageData.fromSnapshot(
            snapshot = relaySnapshot(
                roles = listOf(NodeNetworkRole.RELAY),
                health = health,
                governance = governance,
            ),
            selectedNetworkName = "home",
            backendAvailable = true,
        ) ?: error("relay page projection must exist for a selected network")

        assertTrue(data.relayEnabled)
        assertEquals("home", data.networkName)
        assertSame(health, data.health)
        assertEquals(governance, data.governance)
        assertEquals(setOf(NodePermission.MANAGE_ROLES), data.grantedPermissions)
        val projection = RelayActionPolicy.evaluate(
            backendAvailable = data.backendAvailable,
            grantedPermissions = data.grantedPermissions,
            selectedNetwork = data.networkName,
            requestedNetwork = data.networkName,
        )
        val disableCommand = projection.command(data.networkName, enabled = false)
        assertEquals(false, disableCommand.enabled)
        assertEquals(NodeNetworkRole.RELAY, disableCommand.role)
        val serialized = data.toString().lowercase()
        listOf("payload", "private_key", "session_key", "virtual_ipv6", "display_name").forEach {
            assertFalse(serialized.contains(it))
        }
    }

    @Test
    fun snapshotProjectionFailsClosedWithoutSelectedNetworkAndTargetsEnableWhenInactive() {
        assertNull(
            RelayPageData.fromSnapshot(
                snapshot = relaySnapshot(roles = listOf(NodeNetworkRole.RELAY)),
                selectedNetworkName = "missing",
                backendAvailable = true,
            ),
        )

        val inactive = RelayPageData.fromSnapshot(
            snapshot = relaySnapshot(roles = listOf(NodeNetworkRole.MEMBER)),
            selectedNetworkName = "home",
            backendAvailable = true,
        ) ?: error("relay page projection must exist for a selected network")
        assertFalse(inactive.relayEnabled)
        val enableCommand = RelayActionPolicy.evaluate(
            backendAvailable = inactive.backendAvailable,
            grantedPermissions = inactive.grantedPermissions,
            selectedNetwork = inactive.networkName,
            requestedNetwork = inactive.networkName,
        ).command(inactive.networkName, enabled = true)
        assertEquals(true, enableCommand.enabled)
        assertEquals(NodeNetworkRole.RELAY, enableCommand.role)
    }

    @Test
    fun governanceProjectionKeepsOnlyFiftyMostRecentEvents() {
        val events = (0 until 63).map { index ->
            RelayGovernanceEvent(
                RelayGovernanceAction.RECOVERED,
                null,
                "node-$index",
                null,
                index.toLong(),
                null,
                null,
                "recovered",
                null,
            )
        }
        val projection = RelayGovernanceProjection.from(
            RelayGovernanceAuditSnapshot(4L, events),
        )

        assertEquals(50, projection.shown)
        assertEquals(63, projection.total)
        assertEquals(4L, projection.dropped)
        assertEquals("node-13", projection.events.first().node_id)
        assertEquals("node-62", projection.events.last().node_id)
    }
}

private fun relayHealth() = RelayHealthSnapshot(
    false,
    false,
    listOf(cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthIssue.LISTENER_UNAVAILABLE),
    RelayLifecycleState.RUNNING,
    emptyList(),
    null,
    "203.0.113.9:4433",
    false,
    RelayReachabilityState.UNREACHABLE,
    123L,
    false,
    "listener_unavailable",
    RelayHealthStatus.UNAVAILABLE,
    null,
)

private fun relaySnapshot(
    roles: List<NodeNetworkRole>,
    health: RelayHealthSnapshot? = null,
    governance: RelayGovernanceAuditSnapshot? = null,
): NodeSnapshot {
    val network = NodeNetworkSnapshot(
        null,
        null,
        "home",
        null,
        0L,
        roles,
        null,
        null,
        null,
        0L,
    )
    val service = NodeEmbeddedServiceSnapshot(
        "home",
        null,
        null,
        governance,
        health,
        NodeEmbeddedServiceStatus.RUNNING,
        null,
        NodeEmbeddedServiceStatus.RUNNING,
    )
    return NodeSnapshot(
        null,
        null,
        null,
        null,
        null,
        listOf(service),
        null,
        null,
        null,
        emptyList(),
        emptyList(),
        null,
        null,
        null,
        emptyList(),
        emptyList(),
        emptyList(),
        listOf(network),
        emptyList(),
        NodePermissionSnapshot(
            NodeAccessModel.TRUSTED_CONTROL_PLANE,
            listOf(NodePermission.MANAGE_ROLES),
        ),
        emptyList(),
        1L,
        emptyList(),
        null,
        null,
        null,
        null,
        null,
    )
}
