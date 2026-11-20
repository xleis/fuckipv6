package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TopologySurfaceTest {
    private val snapshot = JSONObject(
        """
        {
          "topology": {
            "revision": 9,
            "generated_at_unix_ms": 1000,
            "viewer": "node-self",
            "truncated": true,
            "boundaries": [{"network_id": "net-1"}, {"network_id": "net-2"}],
            "nodes": [
              {"node_id": "node-self", "is_self": true, "health": "healthy", "networks": [{}, {}]},
              {"node_id": "node-a", "display_name": "小明", "health": "degraded", "networks": [{}]},
              {"display_name": "no-id"},
              "not-an-object"
            ],
            "connections": [
              {"from": "node-self", "to": "node-a", "state": "established", "networks": ["net-1"]},
              {"from": "node-a"}
            ]
          }
        }
        """.trimIndent(),
    )

    @Test
    fun summaryKeepsRevisionFlagsAndCounts() {
        val view = TopologySurface.parse(snapshot)!!
        assertEquals(9L, view.revision)
        assertEquals(1000L, view.generatedAtUnixMs)
        assertTrue(view.nodeTruncated)
        assertEquals(2, view.boundaryCount)
        assertEquals(2, view.nodesTotal)
        assertEquals(1, view.connectionsTotal)
    }

    @Test
    fun selfNodeAndHealthAreMarked() {
        val view = TopologySurface.parse(snapshot)!!
        assertTrue(view.nodes[0].isSelf)
        assertEquals("healthy", view.nodes[0].health)
        assertEquals("小明", view.nodes[1].displayName)
        assertEquals("degraded", view.nodes[1].health)
        assertEquals(2, view.nodes[0].networkCount)
    }

    @Test
    fun clientCapsAreReportedSeparatelyFromTheNodesTruncation() {
        val nodes = (0 until (TopologySurface.MAX_NODES + 5)).joinToString(",") { index ->
            """{"node_id":"n-$index"}"""
        }
        val view = TopologySurface.parse(
            JSONObject("""{"topology":{"revision":1,"truncated":false,"nodes":[$nodes]}}"""),
        )!!
        assertEquals(TopologySurface.MAX_NODES, view.nodes.size)
        assertEquals(TopologySurface.MAX_NODES + 5, view.nodesTotal)
        assertFalse(view.nodeTruncated)
    }

    @Test
    fun missingTopologyIsUnobserved() {
        assertNull(TopologySurface.parse(JSONObject("{}")))
        assertNull(TopologySurface.parse(JSONObject("""{"topology":"nope"}""")))
        val empty = TopologySurface.parse(JSONObject("""{"topology":{"revision":1}}"""))!!
        assertEquals(0, empty.nodesTotal)
        assertEquals(0, empty.connectionsTotal)
    }
}
