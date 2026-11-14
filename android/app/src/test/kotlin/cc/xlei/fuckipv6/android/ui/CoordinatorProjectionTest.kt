package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorCandidateConfigState
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorCandidateHealth
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorCandidateStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorCandidateStatusEntry
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorControlPlaneState
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorControlPlaneStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorEmergencyState
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorPhase
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorQuorumStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorTerm
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeEmbeddedServiceSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeEmbeddedServiceStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkRole
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSnapshot
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CoordinatorProjectionTest {
    @Test
    fun snapshotProjectionExposesLeaderTermAndCandidateHealth() {
        val leader = "node_candidate_healthy"
        val status = CoordinatorCandidateStatus(
            listOf(
                CoordinatorCandidateStatusEntry(
                    null,
                    true,
                    CoordinatorCandidateHealth.HEALTHY,
                    leader,
                    1L,
                    "coordinator.candidate.eligible",
                ),
                CoordinatorCandidateStatusEntry(
                    null,
                    false,
                    CoordinatorCandidateHealth.STALE,
                    "node_candidate_stale",
                    2L,
                    "coordinator.candidate.declaration_expired",
                ),
            ),
            null,
            CoordinatorCandidateConfigState.CURRENT,
            CoordinatorControlPlaneStatus(
                true,
                CoordinatorQuorumStatus(3L, 2L, 2L),
                "coordinator.control_plane.available",
                CoordinatorControlPlaneState.AVAILABLE.wireName(),
            ),
            leader,
            CoordinatorEmergencyState.NONE,
            null,
            123L,
            null,
            CoordinatorTerm(
                "full",
                3L,
                listOf(leader, "node_candidate_stale"),
                200L,
                44L,
                100L,
                leader,
                listOf(leader, "node_recovery"),
                emptyList(),
                "net_home",
                CoordinatorPhase.STABLE,
                4L,
            ),
            null,
            "net_home",
        )

        val data = CoordinatorPageData.fromSnapshot(
            snapshot = coordinatorSnapshot(status),
            selectedNetworkName = "home",
        ) ?: error("Coordinator page projection must exist for a selected network")

        assertEquals("home", data.networkName)
        assertEquals("runtime.control_plane.available", data.stateKey)
        assertTrue(data.governanceWritable)
        assertEquals("coordinator.control_plane.available", data.reasonCode)
        assertEquals(2L, data.receivedVotes)
        assertEquals(2L, data.requiredVotes)
        assertEquals(3L, data.participantCount)
        assertEquals(leader, data.currentLeader)
        assertEquals(4L, data.latestTerm?.term)
        assertEquals(44L, data.latestTerm?.fencingToken)
        assertEquals(leader, data.latestTerm?.leader)
        assertEquals("runtime.coordinator.phase.stable", data.latestTerm?.phaseKey)
        assertEquals(2, data.candidates.size)
        assertEquals("runtime.coordinator.health.healthy", data.candidates.first().healthKey)
        assertTrue(data.candidates.first().eligibleForElection)
        assertEquals("runtime.coordinator.health.stale", data.candidates.last().healthKey)
        assertFalse(data.candidates.last().eligibleForElection)
        assertEquals(
            "coordinator.candidate.declaration_expired",
            data.candidates.last().reasonCode,
        )
    }

    @Test
    fun projectionFailsClosedWithoutNetworkAndShowsUnobservedControlPlane() {
        assertNull(
            CoordinatorPageData.fromSnapshot(
                snapshot = coordinatorSnapshot(null),
                selectedNetworkName = "missing",
            ),
        )

        val unobserved = CoordinatorPageData.fromSnapshot(
            snapshot = coordinatorSnapshot(null),
            selectedNetworkName = "home",
        ) ?: error("selected network must still project an unobserved control plane")

        assertEquals("runtime.control_plane.unobserved", unobserved.stateKey)
        assertFalse(unobserved.governanceWritable)
        assertEquals("coordinator.control_plane.unobserved", unobserved.reasonCode)
        assertNull(unobserved.currentLeader)
        assertNull(unobserved.latestTerm)
        assertTrue(unobserved.candidates.isEmpty())
    }
}

private fun coordinatorSnapshot(status: CoordinatorCandidateStatus?): NodeSnapshot {
    val network = NodeNetworkSnapshot(
        null,
        null,
        "home",
        null,
        0L,
        listOf(NodeNetworkRole.MEMBER),
        null,
        null,
        null,
        0L,
    )
    val service = NodeEmbeddedServiceSnapshot(
        status,
        "home",
        null,
        null,
        null,
        null,
        NodeEmbeddedServiceStatus.STOPPED,
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
        null,
        null,
        null,
        null,
        null,
        emptyList(),
        emptyList(),
        null,
        listOf(network),
        emptyList(),
        null,
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
