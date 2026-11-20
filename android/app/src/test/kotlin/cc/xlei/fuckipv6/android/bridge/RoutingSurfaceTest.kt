package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RoutingSurfaceTest {
    @Test
    fun appliedSummaryIsProjectedWithServerVocabulary() {
        val snapshot = JSONObject(
            """
            {"routing_summaries":[
              {"network_name":"家庭网络",
               "default_action":{"kind":"direct","value":null},
               "effective_target":{"kind":"exit","value":"exit-a"},
               "enabled_rule_count":4,
               "last_applied_revision":7,
               "last_applied_at_unix_ms":1000,
               "status":"applied"}
            ]}
            """.trimIndent(),
        )
        val summary = RoutingSurface.parse(snapshot)!!.single()
        assertEquals("家庭网络", summary.networkName)
        assertEquals("direct", summary.defaultActionKind)
        assertEquals("exit", summary.targetKind)
        assertEquals("exit-a", summary.targetValue)
        assertEquals(4, summary.enabledRuleCount)
        assertEquals(7L, summary.lastAppliedRevision)
        assertEquals(1000L, summary.lastAppliedAtUnixMs)
        assertEquals("applied", summary.status)
        assertNull(summary.problemCode)
    }

    @Test
    fun problemAndUnappliedStateStayVisible() {
        val summary = RoutingSurface.parse(
            JSONObject(
                """{"routing_summaries":[{"network_name":"n","status":"failed","problem":{"code":"routing.error.apply_failed","message_key":"state.error.detail"}}]}""",
            ),
        )!!.single()
        assertEquals("failed", summary.status)
        assertEquals("routing.error.apply_failed", summary.problemCode)
        assertNull(summary.lastAppliedRevision)
        assertEquals(0, summary.enabledRuleCount)
        assertNull(summary.defaultActionKind)
    }

    @Test
    fun missingOrMalformedSummariesAreUnobserved() {
        assertNull(RoutingSurface.parse(JSONObject("{}")))
        assertNull(RoutingSurface.parse(JSONObject("""{"routing_summaries":"nope"}""")))
        assertTrue(RoutingSurface.parse(JSONObject("""{"routing_summaries":[]}"""))!!.isEmpty())
        assertTrue(
            RoutingSurface.parse(JSONObject("""{"routing_summaries":[{"network_name":""},"x"]}"""))!!.isEmpty(),
        )
    }
}
