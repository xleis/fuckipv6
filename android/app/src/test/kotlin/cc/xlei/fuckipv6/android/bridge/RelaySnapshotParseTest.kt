package cc.xlei.fuckipv6.android.bridge

import cc.xlei.fuckipv6.contract.NodeContractV1.NodePermission
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayLifecycleState
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayReachabilityState
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RelaySnapshotParseTest {
    private fun envelope(snapshot: String) =
        """{"schema_version":1,"request_id":"r1","server_time":"t","snapshot":$snapshot}"""

    @Test
    fun relaySurfaceUsesTheAuthoritativeHealthRecord() {
        val body = envelope(
            """
            {
              "networks": [
                {"name": "家庭网络", "roles": ["member", "relay"]},
                {"name": "工作网络", "roles": ["member"]}
              ],
              "embedded_services": [
                {
                  "network_name": "家庭网络",
                  "relay_status": "running",
                  "relay_health": {
                    "status": "healthy",
                    "lifecycle": "running",
                    "network_reachability": "reachable",
                    "listener_bound": true,
                    "listen_addr": "0.0.0.0:4000",
                    "forwarding_observed": true,
                    "authorization_valid": true,
                    "quota_limited": false,
                    "observed_at_unix_ms": 1760000000000,
                    "issues": []
                  }
                }
              ],
              "permissions": {"access_model": "trusted_control_plane", "permissions": ["manage_roles", "view_state"]}
            }
            """.trimIndent(),
        )
        val snapshot = SnapshotJsonParser.parseSnapshot(body).getOrThrow()
        val relay = assertNotNull(snapshot.relay).let { snapshot.relay!! }
        assertEquals("家庭网络", relay.networkName)
        assertTrue(relay.relayEnabled)
        assertTrue(relay.permissions.contains(NodePermission.MANAGE_ROLES))
        val health = assertNotNull(relay.health).let { relay.health!! }
        assertEquals(RelayHealthStatus.HEALTHY, health.status)
        assertEquals(RelayLifecycleState.RUNNING, health.lifecycle)
        assertEquals(RelayReachabilityState.REACHABLE, health.network_reachability)
        assertTrue(health.listener_bound)
        assertEquals("0.0.0.0:4000", health.listen_addr)
        assertTrue(health.forwarding_observed)
        assertTrue(health.authorization_valid)
        assertFalse(health.quota_limited)
        assertEquals(1760000000000L, health.observed_at_unix_ms)
        assertTrue(health.issues.isEmpty())
        // Governance events are not mapped yet; the surface must not claim "none".
        assertNull(relay.governance)
    }

    @Test
    fun networkWithoutTheRelayRoleHasNoRelaySurface() {
        val body = envelope(
            """{"networks":[{"name":"工作网络","roles":["member","exit_node"]}]}""",
        )
        assertNull(SnapshotJsonParser.parseSnapshot(body).getOrThrow().relay)
    }

    @Test
    fun missingOrUnreadableHealthIsDroppedInsteadOfAssumingHealthy() {
        val noHealth = SnapshotJsonParser.parseSnapshot(
            envelope("""{"networks":[{"name":"n","roles":["relay"]}],"embedded_services":[{"network_name":"n"}]}"""),
        ).getOrThrow()
        assertNotNull(noHealth.relay)
        assertNull(noHealth.relay?.health)

        val badStatus = SnapshotJsonParser.parseSnapshot(
            envelope(
                """{"networks":[{"name":"n","roles":["relay"]}],"embedded_services":[{"network_name":"n","relay_health":{"status":"something_new","lifecycle":"running"}}]}""",
            ),
        ).getOrThrow()
        assertNull(badStatus.relay?.health)

        val missingLifecycle = SnapshotJsonParser.parseSnapshot(
            envelope(
                """{"networks":[{"name":"n","roles":["relay"]}],"embedded_services":[{"network_name":"n","relay_health":{"status":"healthy"}}]}""",
            ),
        ).getOrThrow()
        assertNull(missingLifecycle.relay?.health)
    }

    @Test
    fun unknownPermissionsAndRolesAreIgnored() {
        val body = envelope(
            """
            {"networks":[{"name":"n","roles":["relay","future_role"]}],
             "permissions":{"access_model":"trusted_control_plane","permissions":["view_state","future_permission"]}}
            """.trimIndent(),
        )
        val relay = SnapshotJsonParser.parseSnapshot(body).getOrThrow().relay!!
        assertEquals("n", relay.networkName)
        assertEquals(setOf(NodePermission.VIEW_STATE), relay.permissions)
    }
}
