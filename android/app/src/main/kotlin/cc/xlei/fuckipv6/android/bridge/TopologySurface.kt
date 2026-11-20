package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * One node in the topology.
 *
 * `networkCount` is a count, not the list: the topology projection may hide
 * individual networks, and the Android list would otherwise imply it knows which
 * ones.
 */
data class TopologyNodeView(
    val nodeId: String,
    val displayName: String?,
    val isSelf: Boolean,
    val health: String?,
    val networkCount: Int,
)

data class TopologyConnectionView(
    val from: String,
    val to: String,
    val state: String?,
    val networkCount: Int,
)

/**
 * A bounded summary of the topology.
 *
 * The node itself caps the topology (`truncated`); the client caps what it
 * renders and reports `nodesTotal` / `connectionsTotal` separately, so a short
 * list is never mistaken for a small network.
 */
data class TopologyView(
    val revision: Long,
    val generatedAtUnixMs: Long?,
    val nodeTruncated: Boolean,
    val boundaryCount: Int,
    val nodes: List<TopologyNodeView>,
    val nodesTotal: Int,
    val connections: List<TopologyConnectionView>,
    val connectionsTotal: Int,
)

object TopologySurface {
    const val MAX_NODES = 50
    const val MAX_CONNECTIONS = 100

    fun parse(snapshot: JSONObject): TopologyView? {
        val topology = snapshot.optJSONObject("topology") ?: return null
        val rawNodes = topology.optJSONArray("nodes") ?: org.json.JSONArray()
        val rawConnections = topology.optJSONArray("connections") ?: org.json.JSONArray()
        val nodes = (0 until rawNodes.length()).mapNotNull { index ->
            rawNodes.optJSONObject(index)?.let(::node)
        }
        val connections = (0 until rawConnections.length()).mapNotNull { index ->
            rawConnections.optJSONObject(index)?.let(::connection)
        }
        return TopologyView(
            revision = topology.longOrNull("revision") ?: 0L,
            generatedAtUnixMs = topology.longOrNull("generated_at_unix_ms"),
            nodeTruncated = topology.optBoolean("truncated", false),
            boundaryCount = topology.optJSONArray("boundaries")?.length() ?: 0,
            nodes = nodes.take(MAX_NODES),
            nodesTotal = nodes.size,
            connections = connections.take(MAX_CONNECTIONS),
            connectionsTotal = connections.size,
        )
    }

    private fun node(json: JSONObject): TopologyNodeView? {
        val nodeId = json.stringOrNull("node_id") ?: return null
        return TopologyNodeView(
            nodeId = nodeId,
            displayName = json.stringOrNull("display_name"),
            isSelf = json.optBoolean("is_self", false),
            health = json.stringOrNull("health"),
            networkCount = json.optJSONArray("networks")?.length() ?: 0,
        )
    }

    private fun connection(json: JSONObject): TopologyConnectionView? {
        val from = json.stringOrNull("from") ?: return null
        val to = json.stringOrNull("to") ?: return null
        return TopologyConnectionView(
            from = from,
            to = to,
            state = json.stringOrNull("state"),
            networkCount = json.optJSONArray("networks")?.length() ?: 0,
        )
    }
}
