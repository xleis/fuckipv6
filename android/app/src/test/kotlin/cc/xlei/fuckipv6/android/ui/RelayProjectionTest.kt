package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.contract.NodeContractV1.NodeCommandKind
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkRole
import cc.xlei.fuckipv6.contract.NodeContractV1.NodePermission
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
        val health = RelayHealthSnapshot(
            false,
            false,
            emptyList(),
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

        val projection = RelayHealthProjection.from(health)

        assertFalse(projection.listenerBound)
        assertEquals("203.0.113.9:4433", projection.listenAddr)
        assertEquals("relay.health.unavailable", projection.statusKey)
        assertEquals("relay.lifecycle.running", projection.lifecycleKey)
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
