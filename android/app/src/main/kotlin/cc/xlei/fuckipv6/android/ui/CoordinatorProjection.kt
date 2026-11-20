package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.CoordinatorCandidateView
import cc.xlei.fuckipv6.android.bridge.CoordinatorSurface
import cc.xlei.fuckipv6.android.bridge.CoordinatorTermView
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorCandidateHealth
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorCandidateStatus
import cc.xlei.fuckipv6.contract.NodeContractV1.CoordinatorPhase
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSnapshot

data class CoordinatorCandidateProjection(
    val nodeId: String,
    val priority: Long?,
    val healthKey: String,
    val eligibleForElection: Boolean,
    val reasonCode: String,
)

data class CoordinatorTermProjection(
    val term: Long,
    val fencingToken: Long,
    val leader: String,
    val phaseKey: String,
)

data class CoordinatorPageData(
    val networkName: String,
    val stateKey: String,
    val governanceWritable: Boolean,
    val reasonCode: String,
    val receivedVotes: Long,
    val requiredVotes: Long,
    val participantCount: Long,
    val currentLeader: String?,
    val latestTerm: CoordinatorTermProjection?,
    val candidates: List<CoordinatorCandidateProjection>,
) {
    companion object {
        fun fromSnapshot(
            snapshot: NodeSnapshot,
            selectedNetworkName: String,
        ): CoordinatorPageData? {
            if (selectedNetworkName.isBlank()) return null
            val network = snapshot.networks.orEmpty().firstOrNull {
                it.name == selectedNetworkName
            } ?: return null
            val service = snapshot.embedded_services.orEmpty().firstOrNull {
                it.network_name == selectedNetworkName
            }
            return fromStatus(network.name, service?.coordinator_candidates)
        }

        fun fromStatus(
            networkName: String,
            status: CoordinatorCandidateStatus?,
        ): CoordinatorPageData = fromBridge(
            CoordinatorSurface(
                networkName = networkName,
                controlPlaneState = status?.control_plane?.state,
                governanceWritable = status?.control_plane?.governance_writable == true,
                reasonCode = status?.control_plane?.reason_code,
                receivedVotes = status?.control_plane?.quorum?.received_votes ?: 0L,
                requiredVotes = status?.control_plane?.quorum?.required_votes ?: 0L,
                participantCount = status?.control_plane?.quorum?.participant_count ?: 0L,
                currentLeader = status?.current_leader,
                latestTerm = status?.latest_term?.let { term ->
                    val number = term.term
                    val token = term.fencing_token
                    val leader = term.leader
                    if (number == null || token == null || leader.isNullOrBlank()) {
                        null
                    } else {
                        CoordinatorTermView(number, token, leader, term.phase?.wireName())
                    }
                },
                candidates = status?.candidates.orEmpty().mapNotNull { candidate ->
                    val nodeId = candidate.node_id?.takeIf(String::isNotBlank)
                        ?: return@mapNotNull null
                    CoordinatorCandidateView(
                        nodeId = nodeId,
                        priority = candidate.priority,
                        health = (candidate.health ?: CoordinatorCandidateHealth.MISSING).wireName(),
                        eligibleForElection = candidate.eligible_for_election == true,
                        reasonCode = candidate.reason_code,
                    )
                },
            ),
        )

        /** Bridge-level entry point: same semantics, plain models instead of DTOs. */
        fun fromBridge(surface: CoordinatorSurface): CoordinatorPageData {
            val stateKey = when (surface.controlPlaneState) {
                "available" -> "runtime.control_plane.available"
                null -> "runtime.control_plane.unobserved"
                else -> "runtime.control_plane.limited"
            }
            val candidates = surface.candidates.map { candidate ->
                CoordinatorCandidateProjection(
                    nodeId = candidate.nodeId,
                    priority = candidate.priority,
                    healthKey = "runtime.coordinator.health.${candidate.health ?: "missing"}",
                    eligibleForElection = candidate.eligibleForElection,
                    reasonCode = candidate.reasonCode?.takeIf(String::isNotBlank)
                        ?: "coordinator.candidate.health_unknown",
                )
            }
            val latestTerm = surface.latestTerm?.let { term ->
                CoordinatorTermProjection(
                    term = term.term,
                    fencingToken = term.fencingToken,
                    leader = term.leader,
                    phaseKey = coordinatorPhaseKey(term.phase),
                )
            }
            return CoordinatorPageData(
                networkName = surface.networkName,
                stateKey = stateKey,
                governanceWritable = surface.governanceWritable,
                reasonCode = surface.reasonCode?.takeIf(String::isNotBlank)
                    ?: "coordinator.control_plane.unobserved",
                receivedVotes = surface.receivedVotes,
                requiredVotes = surface.requiredVotes,
                participantCount = surface.participantCount,
                currentLeader = surface.currentLeader?.takeIf(String::isNotBlank),
                latestTerm = latestTerm,
                candidates = candidates,
            )
        }

        private fun coordinatorPhaseKey(phase: String?): String =
            "runtime.coordinator.phase.${phase ?: "unknown"}"
    }
}
