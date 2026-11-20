package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * Coordinator-relevant slice of the authoritative snapshot, in plain models.
 *
 * The Coordinator surface only reads leadership, quorum, candidate health and
 * the latest term, so the bridge exposes exactly those and keeps the generated
 * DTO graph out of the parser. Anything the node does not publish stays null;
 * the page then shows its own "unobserved" state instead of a guess.
 */
data class CoordinatorSurface(
    val networkName: String,
    val controlPlaneState: String?,
    val governanceWritable: Boolean,
    val reasonCode: String?,
    val receivedVotes: Long,
    val requiredVotes: Long,
    val participantCount: Long,
    val currentLeader: String?,
    val latestTerm: CoordinatorTermView?,
    val candidates: List<CoordinatorCandidateView>,
)

data class CoordinatorTermView(
    val term: Long,
    val fencingToken: Long,
    val leader: String,
    val phase: String?,
)

data class CoordinatorCandidateView(
    val nodeId: String,
    val priority: Long?,
    val health: String?,
    val eligibleForElection: Boolean,
    val reasonCode: String?,
)

/** Extracts the Coordinator slice from a snapshot object. */
internal fun coordinatorSurface(snapshot: JSONObject): CoordinatorSurface? {
    val service = snapshot.optJSONArray("embedded_services")
        ?.let { array ->
            (0 until array.length())
                .mapNotNull { index -> array.optJSONObject(index) }
                .firstOrNull { it.optJSONObject("coordinator_candidates") != null }
        }
        ?: return null
    val status = service.optJSONObject("coordinator_candidates") ?: return null
    val networkName = service.stringOrNull("network_name") ?: return null
    val controlPlane = status.optJSONObject("control_plane")
    val quorum = controlPlane?.optJSONObject("quorum")
    val term = status.optJSONObject("latest_term")?.let { value ->
        val number = value.longOrNull("term") ?: return@let null
        val token = value.longOrNull("fencing_token") ?: return@let null
        val leader = value.stringOrNull("leader") ?: return@let null
        CoordinatorTermView(number, token, leader, value.stringOrNull("phase"))
    }
    val candidates = status.optJSONArray("candidates")?.let { array ->
        (0 until array.length()).mapNotNull { index ->
            val candidate = array.optJSONObject(index) ?: return@mapNotNull null
            val nodeId = candidate.stringOrNull("node_id") ?: return@mapNotNull null
            CoordinatorCandidateView(
                nodeId = nodeId,
                priority = candidate.longOrNull("priority"),
                health = candidate.stringOrNull("health"),
                eligibleForElection = candidate.optBoolean("eligible_for_election", false),
                reasonCode = candidate.stringOrNull("reason_code"),
            )
        }
    }.orEmpty()
    return CoordinatorSurface(
        networkName = networkName,
        controlPlaneState = controlPlane?.stringOrNull("state"),
        governanceWritable = controlPlane?.optBoolean("governance_writable", false) ?: false,
        reasonCode = controlPlane?.stringOrNull("reason_code"),
        receivedVotes = quorum?.longOrNull("received_votes") ?: 0L,
        requiredVotes = quorum?.longOrNull("required_votes") ?: 0L,
        participantCount = quorum?.longOrNull("participant_count") ?: 0L,
        currentLeader = status.stringOrNull("current_leader"),
        latestTerm = term,
        candidates = candidates,
    )
}

internal fun JSONObject.stringOrNull(key: String): String? {
    if (!has(key) || isNull(key)) return null
    return optString(key, "").takeIf { it.isNotEmpty() }
}

internal fun JSONObject.longOrNull(key: String): Long? {
    if (!has(key) || isNull(key)) return null
    return when (val value = opt(key)) {
        is Number -> value.toLong()
        is String -> value.toLongOrNull()
        else -> null
    }
}