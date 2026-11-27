package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TrafficPolicySurfaceTest {
    @Test
    fun buildsAllSevenAuthoritativeTrafficPolicyCommands() {
        assertEquals("get_traffic_policy", TrafficPolicySurface.getPolicyCommand().getString("command"))
        assertEquals("get_traffic_policy_history", TrafficPolicySurface.getHistoryCommand().getString("command"))

        val draft = TrafficPolicySurface.buildDraftJson(
            defaultActionKind = "direct",
            defaultActionTarget = null,
            rules = listOf(
                TrafficRuleItem("r1", "Google", true, "domain_suffix", "google.com", "exit", "node-hk", fixedExit = true),
            ),
        )
        assertEquals("preview_traffic_policy", TrafficPolicySurface.previewCommand(1L, draft)!!.getString("command"))
        assertEquals("apply_traffic_policy", TrafficPolicySurface.applyCommand(1L, draft, true)!!.getString("command"))
        assertEquals("preview_traffic_policy_rollback", TrafficPolicySurface.previewRollbackCommand(2L, 1L)!!.getString("command"))
        assertEquals("rollback_traffic_policy", TrafficPolicySurface.rollbackCommand(2L, 1L, true)!!.getString("command"))
        val testCmd = TrafficPolicySurface.testCommand(JSONObject(), JSONObject(), "www.google.com")
        assertNotNull(testCmd)
        assertEquals("test_traffic_policy", testCmd!!.getString("command"))
        assertNull(TrafficPolicySurface.testCommand(JSONObject(), JSONObject(), "   "))
    }

    @Test
    fun parsesPolicyDocumentPreviewTestAndHistory() {
        val doc = TrafficPolicySurface.parseDocument(
            JSONObject("""{"policy":{"revision":4,"default_action":{"kind":"direct"},"rules":[{"id":"r1","name":"YouTube","enabled":true,"matcher":{"domain_suffix":"youtube.com"},"action":{"kind":"exit","value":"node-hk"},"fixed_exit":true}]}}"""),
        )!!
        assertEquals(4L, doc.revision)
        assertEquals("direct", doc.defaultActionKind)
        assertEquals(1, doc.rules.size)
        assertTrue(doc.rules[0].fixedExit)

        val preview = TrafficPolicySurface.parsePreview(
            JSONObject("""{"current_revision":4,"candidate_revision":5,"changes":[{}],"risks":[{"kind":"fixed_exit"}]}"""),
        )!!
        assertEquals(5L, preview.candidateRevision)
        assertTrue(preview.requiresRiskConfirm)

        val testView = TrafficPolicySurface.parseTest(
            JSONObject("""{"current":{"action":{"kind":"direct"}},"candidate":{"action":{"kind":"exit"},"matched_rule_id":"r1"}}"""),
        )!!
        assertTrue(testView.changed)
        assertEquals("r1", testView.matchedRuleId)

        val history = TrafficPolicySurface.parseHistory(
            JSONObject("""{"entries":[{"revision":3,"action":"apply","recorded_at_unix_ms":1000}]}"""),
        )
        assertEquals(1, history.size)
        assertEquals(3L, history[0].revision)
    }
}
