package cc.xlei.fuckipv6.android.ui

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
        ): CoordinatorPageData {
            val controlPlane = status?.control_plane
            val stateKey = when (controlPlane?.state) {
                "available" -> "runtime.control_plane.available"
                null -> "runtime.control_plane.unobserved"
                else -> "runtime.control_plane.limited"
            }
            val candidates = status?.candidates.orEmpty().mapNotNull { candidate ->
                val nodeId = candidate.node_id?.takeIf(String::isNotBlank)
                    ?: return@mapNotNull null
                val health = candidate.health ?: CoordinatorCandidateHealth.MISSING
                CoordinatorCandidateProjection(
                    nodeId = nodeId,
                    priority = candidate.priority,
                    healthKey = "runtime.coordinator.health.${health.wireName()}",
                    eligibleForElection = candidate.eligible_for_election == true,
                    reasonCode = candidate.reason_code?.takeIf(String::isNotBlank)
                        ?: "coordinator.candidate.health_unknown",
                )
            }
            val latestTerm = status?.latest_term?.let { term ->
                val termNumber = term.term ?: return@let null
                val fencingToken = term.fencing_token ?: return@let null
                val leader = term.leader?.takeIf(String::isNotBlank) ?: return@let null
                CoordinatorTermProjection(
                    term = termNumber,
                    fencingToken = fencingToken,
                    leader = leader,
                    phaseKey = coordinatorPhaseKey(term.phase),
                )
            }
            return CoordinatorPageData(
                networkName = networkName,
                stateKey = stateKey,
                governanceWritable = controlPlane?.governance_writable == true,
                reasonCode = controlPlane?.reason_code?.takeIf(String::isNotBlank)
                    ?: "coordinator.control_plane.unobserved",
                receivedVotes = controlPlane?.quorum?.received_votes ?: 0L,
                requiredVotes = controlPlane?.quorum?.required_votes ?: 0L,
                participantCount = controlPlane?.quorum?.participant_count ?: 0L,
                currentLeader = status?.current_leader?.takeIf(String::isNotBlank),
                latestTerm = latestTerm,
                candidates = candidates,
            )
        }

        private fun coordinatorPhaseKey(phase: CoordinatorPhase?): String =
            "runtime.coordinator.phase.${phase?.wireName() ?: "unknown"}"
    }
}
