package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONObject

/** One join application as the node lists it for a reviewer. */
data class JoinRequestView(
    val requestId: String,
    val networkId: String?,
    val applicantNodeId: String?,
    val applicantDisplayName: String?,
    val state: String?,
    val revision: Long?,
    val createdAtUnixMs: Long?,
    val expiresAtUnixMs: Long?,
) {
    val isPending: Boolean
        get() = state == "pending"
}

/** One revision-checked review decision. */
data class JoinReviewRequest(
    val networkId: String,
    val requestId: String,
    val action: String,
    val expectedRevision: Long,
    val note: String? = null,
)

data class JoinReviewOutcomeView(
    val requestId: String?,
    val state: String?,
    val revision: Long?,
)

object JoinRequestSurface {
    val ACTIONS = setOf("approve", "reject", "request_information")

    fun listCommand(networkId: String): JSONObject = JSONObject()
        .put("command", "list_join_requests")
        .put("payload", JSONObject().put("network_id", networkId))

    /** Encodes a review; unknown actions or blank ids never reach the wire. */
    fun reviewCommand(request: JoinReviewRequest): JSONObject? {
        if (request.action !in ACTIONS) return null
        if (request.networkId.isBlank() || request.requestId.isBlank()) return null
        if (request.expectedRevision < 0) return null
        val review = JSONObject()
            .put("request_id", request.requestId)
            .put("expected_revision", request.expectedRevision)
            .put("action", request.action)
        request.note?.takeIf { it.isNotBlank() }?.let { review.put("note", it) }
        return JSONObject()
            .put("command", "review_join_request")
            .put(
                "payload",
                JSONObject()
                    .put("network_id", request.networkId)
                    .put("review", review),
            )
    }

    /**
     * Parses the `join_requests` result. Reason and notes stay private: only the
     * applicant's published profile, state and revision are read.
     */
    fun parseList(data: JSONObject?): List<JoinRequestView> {
        if (data == null) return emptyList()
        val array = data.optJSONArray("requests") ?: return emptyList()
        return (0 until array.length()).mapNotNull { index ->
            array.optJSONObject(index)?.let(::request)
        }
    }

    fun parseReview(data: JSONObject?): JoinReviewOutcomeView? {
        if (data == null) return null
        return JoinReviewOutcomeView(
            requestId = data.stringOrNull("request_id"),
            state = data.stringOrNull("state"),
            revision = data.longOrNull("revision"),
        )
    }

    private fun request(json: JSONObject): JoinRequestView? {
        val requestId = json.stringOrNull("request_id") ?: return null
        val applicant = json.optJSONObject("applicant")
        return JoinRequestView(
            requestId = requestId,
            networkId = json.stringOrNull("network_id"),
            applicantNodeId = applicant?.stringOrNull("node_id"),
            applicantDisplayName = applicant?.stringOrNull("display_name"),
            state = json.stringOrNull("state"),
            revision = json.longOrNull("revision"),
            createdAtUnixMs = json.longOrNull("created_at_unix_ms"),
            expiresAtUnixMs = json.longOrNull("expires_at_unix_ms"),
        )
    }
}
