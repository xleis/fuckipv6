package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class JoinRequestSurfaceTest {
    private val payload = JSONObject(
        """
        {
          "network_id": "net-1",
          "requests": [
            {
              "request_id": "req-1",
              "network_id": "net-1",
              "applicant": {"node_id": "node-a", "display_name": "小明"},
              "reason": "private note that must not be rendered",
              "state": "pending",
              "revision": 3,
              "created_at_unix_ms": 10,
              "expires_at_unix_ms": 20
            },
            {"request_id": "", "state": "pending"},
            "not-an-object"
          ]
        }
        """.trimIndent(),
    )

    @Test
    fun listKeepsOnlyThePublicApplicantProjection() {
        val requests = JoinRequestSurface.parseList(payload)
        assertEquals(1, requests.size)
        val first = requests.single()
        assertEquals("req-1", first.requestId)
        assertEquals("node-a", first.applicantNodeId)
        assertEquals("小明", first.applicantDisplayName)
        assertEquals(3L, first.revision)
        assertTrue(first.isPending)
        assertFalse(first.toString().contains("private note"))
    }

    @Test
    fun reviewCommandCarriesRevisionAndClosedActionSet() {
        val command = JoinRequestSurface.reviewCommand(
            JoinReviewRequest("net-1", "req-1", "approve", expectedRevision = 3),
        )!!
        assertEquals("review_join_request", command.getString("command"))
        val review = command.getJSONObject("payload").getJSONObject("review")
        assertEquals("req-1", review.getString("request_id"))
        assertEquals(3L, review.getLong("expected_revision"))
        assertEquals("approve", review.getString("action"))
        assertFalse(review.has("note"))

        assertNull(JoinRequestSurface.reviewCommand(JoinReviewRequest("net-1", "req-1", "ban", 3)))
        assertNull(JoinRequestSurface.reviewCommand(JoinReviewRequest("net-1", "", "approve", 3)))
        assertNull(JoinRequestSurface.reviewCommand(JoinReviewRequest("", "req-1", "approve", 3)))
        assertNull(JoinRequestSurface.reviewCommand(JoinReviewRequest("net-1", "req-1", "approve", -1)))
    }

    @Test
    fun reviewOutcomeAndEmptyPayloadsStayHonest() {
        val outcome = JoinRequestSurface.parseReview(
            JSONObject("""{"request_id":"req-1","state":"approved","revision":4}"""),
        )!!
        assertEquals("approved", outcome.state)
        assertEquals(4L, outcome.revision)

        assertTrue(JoinRequestSurface.parseList(null).isEmpty())
        assertTrue(JoinRequestSurface.parseList(JSONObject("{}")).isEmpty())
        assertNull(JoinRequestSurface.parseReview(null))
    }

    @Test
    fun batchCommandRefusesEmptyDuplicateAndInvalidBatches() {
        val reviews = listOf(
            JoinReviewRequest("net-1", "req-1", "approve", 1),
            JoinReviewRequest("net-1", "req-2", "reject", 2, note = "理由"),
        )
        val command = JoinRequestSurface.batchReviewCommand("net-1", reviews)!!
        assertEquals("batch_review_join_requests", command.getString("command"))
        val payload = command.getJSONObject("payload")
        assertEquals("net-1", payload.getString("network_id"))
        val encoded = payload.getJSONArray("reviews")
        assertEquals(2, encoded.length())
        assertEquals("req-1", encoded.getJSONObject(0).getString("request_id"))
        assertEquals(1L, encoded.getJSONObject(0).getLong("expected_revision"))
        assertEquals("approve", encoded.getJSONObject(0).getString("action"))
        assertEquals("理由", encoded.getJSONObject(1).getString("note"))

        assertNull(JoinRequestSurface.batchReviewCommand("net-1", emptyList()))
        assertNull(JoinRequestSurface.batchReviewCommand(" ", reviews))
        assertNull(
            JoinRequestSurface.batchReviewCommand(
                "net-1",
                listOf(reviews[0], reviews[0].copy(action = "reject")),
            ),
        )
        assertNull(
            JoinRequestSurface.batchReviewCommand(
                "net-1",
                listOf(reviews[0].copy(action = "ban")),
            ),
        )
        assertNull(
            JoinRequestSurface.batchReviewCommand(
                "net-1",
                listOf(reviews[0].copy(expectedRevision = -1)),
            ),
        )
    }

    @Test
    fun batchOutcomeKeepsEveryDecisionWithItsState() {
        val result = JoinRequestSurface.parseBatchReview(
            JSONObject(
                """{"network_id":"net-1","reviews":[
                     {"request_id":"req-1","state":"approved","revision":2},
                     {"request_id":"req-2","state":"rejected","revision":3},
                     "not-an-object"]}""".trimIndent(),
            ),
        )!!
        assertEquals("net-1", result.networkId)
        assertEquals(2, result.outcomes.size)
        assertEquals("approved", result.outcomes[0].state)
        assertEquals(3L, result.outcomes[1].revision)

        assertNull(JoinRequestSurface.parseBatchReview(null))
        assertNull(JoinRequestSurface.parseBatchReview(JSONObject("{}")))
    }
}
