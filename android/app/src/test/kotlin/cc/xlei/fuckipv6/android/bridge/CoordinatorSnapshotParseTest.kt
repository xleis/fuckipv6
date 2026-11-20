package cc.xlei.fuckipv6.android.bridge

import cc.xlei.fuckipv6.android.ui.CoordinatorPageData
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CoordinatorSnapshotParseTest {
    private fun envelope(snapshot: String) =
        """{"schema_version":1,"request_id":"r1","server_time":"t","snapshot":$snapshot}"""

    private val full = envelope(
        """
        {
          "networks": [{"name": "家庭网络", "roles": ["member"]}],
          "embedded_services": [
            {
              "network_name": "家庭网络",
              "coordinator_candidates": {
                "network_id": "net-1",
                "generated_at_unix_ms": 1760000000000,
                "config_state": "current",
                "candidates": [
                  {"node_id": "node-a", "priority": 1, "health": "healthy", "eligible_for_election": true, "reason_code": "coordinator.candidate.healthy"},
                  {"node_id": "node-b", "priority": 2, "health": "stale", "eligible_for_election": false, "reason_code": "coordinator.candidate.declaration_expired"}
                ],
                "current_leader": "node-a",
                "latest_term": {"term": 3, "fencing_token": 3, "leader": "node-a", "phase": "stable"},
                "control_plane": {
                  "state": "available",
                  "governance_writable": true,
                  "reason_code": "coordinator.control_plane.available",
                  "quorum": {"received_votes": 2, "required_votes": 2, "participant_count": 3}
                }
              }
            }
          ]
        }
        """.trimIndent(),
    )

    @Test
    fun coordinatorSurfaceMirrorsLeadershipQuorumAndCandidates() {
        val surface = SnapshotJsonParser.parseSnapshot(full).getOrThrow().coordinator!!
        assertEquals("家庭网络", surface.networkName)
        assertEquals("available", surface.controlPlaneState)
        assertTrue(surface.governanceWritable)
        assertEquals(2L, surface.receivedVotes)
        assertEquals(2L, surface.requiredVotes)
        assertEquals(3L, surface.participantCount)
        assertEquals("node-a", surface.currentLeader)
        assertEquals(3L, surface.latestTerm?.term)
        assertEquals(3L, surface.latestTerm?.fencingToken)
        assertEquals("stable", surface.latestTerm?.phase)
        assertEquals(2, surface.candidates.size)

        val page = CoordinatorPageData.fromBridge(surface)
        assertEquals("runtime.control_plane.available", page.stateKey)
        assertTrue(page.governanceWritable)
        assertEquals("node-a", page.currentLeader)
        assertEquals("runtime.coordinator.phase.stable", page.latestTerm?.phaseKey)
        assertEquals("runtime.coordinator.health.healthy", page.candidates[0].healthKey)
        assertTrue(page.candidates[0].eligibleForElection)
        assertEquals("runtime.coordinator.health.stale", page.candidates[1].healthKey)
        assertFalse(page.candidates[1].eligibleForElection)
    }

    @Test
    fun missingControlPlaneIsUnobservedRatherThanAvailable() {
        val body = envelope(
            """
            {"networks":[{"name":"n","roles":["member"]}],
             "embedded_services":[{"network_name":"n","coordinator_candidates":{"network_id":"net-1","config_state":"pending","candidates":[]}}]}
            """.trimIndent(),
        )
        val surface = SnapshotJsonParser.parseSnapshot(body).getOrThrow().coordinator!!
        assertNull(surface.controlPlaneState)
        val page = CoordinatorPageData.fromBridge(surface)
        assertEquals("runtime.control_plane.unobserved", page.stateKey)
        assertEquals("coordinator.control_plane.unobserved", page.reasonCode)
        assertFalse(page.governanceWritable)
        assertEquals(0L, page.requiredVotes)
        assertNull(page.latestTerm)
    }

    @Test
    fun incompleteTermOrCandidateIsDropped() {
        val body = envelope(
            """
            {"embedded_services":[{"network_name":"n","coordinator_candidates":{
              "network_id":"net-1","config_state":"current",
              "latest_term":{"term":2,"leader":"node-a"},
              "candidates":[{"node_id":"","priority":1},{"priority":2}]}}]}
            """.trimIndent(),
        )
        val surface = SnapshotJsonParser.parseSnapshot(body).getOrThrow().coordinator!!
        assertNull(surface.latestTerm)
        assertTrue(surface.candidates.isEmpty())
    }

    @Test
    fun networkWithoutCoordinatorServiceHasNoSurface() {
        val body = envelope("""{"networks":[{"name":"n","roles":["relay"]}],"embedded_services":[]}""")
        assertNull(SnapshotJsonParser.parseSnapshot(body).getOrThrow().coordinator)
    }

    @Test
    fun dtoPathAndBridgePathAgree() {
        val surface = SnapshotJsonParser.parseSnapshot(full).getOrThrow().coordinator!!
        val fromBridge = CoordinatorPageData.fromBridge(surface)
        assertEquals("家庭网络", fromBridge.networkName)
        assertEquals(fromBridge.candidates.size, surface.candidates.size)
        assertEquals(
            fromBridge.latestTerm?.fencingToken,
            surface.latestTerm?.fencingToken,
        )
    }
}
