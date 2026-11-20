package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TrafficActivitySurfaceTest {
    private val snapshot = JSONObject(
        """
        {
          "networks": [
            {
              "name": "家庭网络",
              "traffic": {
                "tun_rx_packets": 10, "tun_rx_bytes": 1024,
                "tun_tx_packets": 20, "tun_tx_bytes": 2048,
                "p2p_rx_packets": 30, "p2p_rx_bytes": 4096,
                "p2p_tx_packets": 40, "p2p_tx_bytes": 8192,
                "error_count": 2
              }
            },
            {"name": "工作网络"}
          ],
          "recent_activity": [
            {"audit_id": "a-1", "action": "network.start", "resource": "network/net-1",
             "outcome": "succeeded", "category": "runtime", "severity": "info",
             "occurred_at_unix_ms": 100, "summary": "已启动", "network_id": "net-1"},
            {"audit_id": "a-2", "action": "node.rotate", "resource": "node/self",
             "outcome": "succeeded", "occurred_at_unix_ms": 200},
            {"audit_id": "a-3", "action": "network.stop", "resource": "network/net-2",
             "outcome": "failed", "occurred_at_unix_ms": 300, "network_id": "net-2"},
            {"action": "no-audit-id", "resource": "x"},
            "not-an-object"
          ]
        }
        """.trimIndent(),
    )

    @Test
    fun trafficKeepsTheReportedCountersForTheSelectedNetwork() {
        val traffic = TrafficSurface.parse(snapshot, "家庭网络")!!
        assertEquals(10L, traffic.tunRxPackets)
        assertEquals(1024L, traffic.tunRxBytes)
        assertEquals(2048L, traffic.tunTxBytes)
        assertEquals(8192L, traffic.p2pTxBytes)
        assertEquals(2L, traffic.errorCount)
    }

    @Test
    fun trafficIsAbsentRatherThanZeroWhenTheNodeDidNotReportIt() {
        assertNull(TrafficSurface.parse(snapshot, "工作网络"))
        assertNull(TrafficSurface.parse(snapshot, "不存在的网络"))
        assertNull(TrafficSurface.parse(JSONObject("{}"), "家庭网络"))
    }

    @Test
    fun activityIsNewestFirstAndFilteredByNetworkPlusGlobal() {
        val feed = ActivitySurface.parse(snapshot, "net-1")!!
        assertEquals(listOf("a-2", "a-1"), feed.entries.map { it.auditId })
        assertEquals("已启动", feed.entries[1].summary)
        assertEquals("runtime", feed.entries[1].category)
        assertNull(feed.entries[0].summary)
    }

    @Test
    fun entriesWithoutStableIdentityAreDropped() {
        val feed = ActivitySurface.parse(snapshot, null)!!
        assertFalse(feed.entries.any { it.action == "no-audit-id" })
        assertEquals(3, feed.entries.size)
        assertEquals("a-3", feed.entries.first().auditId)
        assertEquals("failed", feed.entries.first().outcome)
    }

    @Test
    fun missingOrWrongTypedActivityIsUnobserved() {
        assertNull(ActivitySurface.parse(JSONObject("{}"), "net-1"))
        assertNull(ActivitySurface.parse(JSONObject("""{"recent_activity":"nope"}"""), "net-1"))
        val empty = ActivitySurface.parse(JSONObject("""{"recent_activity":[]}"""), "net-1")!!
        assertTrue(empty.entries.isEmpty())
    }

    @Test
    fun activityFeedIsBounded() {
        val entries = (0 until 80).joinToString(",") { index ->
            """{"audit_id":"a-$index","action":"x","resource":"r","occurred_at_unix_ms":$index}"""
        }
        val feed = ActivitySurface.parse(
            JSONObject("""{"recent_activity":[$entries]}"""),
            "net-1",
        )!!
        assertEquals(ActivitySurface.MAX_ENTRIES, feed.entries.size)
        assertEquals("a-79", feed.entries.first().auditId)
    }
}
