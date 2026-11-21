package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONObject

/**
 * The node's per-network client blacklist.
 *
 * `changed == false` on a mutation means the node treated the request as
 * idempotent (the client was already in that state), which is why the UI never
 * reports "banned" from its own intent — it re-reads this list.
 */
data class BlacklistView(
    val networkName: String,
    val clients: List<String>,
    val changed: Boolean,
)

object BlacklistSurface {
    fun statusCommand(networkName: String): JSONObject? = command(
        name = "get_blacklist",
        networkName = networkName,
        clientId = null,
    )

    fun banCommand(networkName: String, clientId: String): JSONObject? = command(
        name = "ban_client",
        networkName = networkName,
        clientId = clientId,
    )

    fun unbanCommand(networkName: String, clientId: String): JSONObject? = command(
        name = "unban_client",
        networkName = networkName,
        clientId = clientId,
    )

    fun parse(data: JSONObject?): BlacklistView? {
        if (data == null) return null
        val networkName = data.stringOrNull("network_name") ?: return null
        val raw = data.opt("clients") ?: JSONArray()
        val clients = when (raw) {
            is JSONArray -> (0 until raw.length()).mapNotNull { index ->
                raw.optString(index, "").takeIf { it.isNotEmpty() }
            }
            JSONObject.NULL -> emptyList()
            else -> return null
        }
        return BlacklistView(
            networkName = networkName,
            clients = clients,
            changed = data.optBoolean("changed", false),
        )
    }

    private fun command(name: String, networkName: String, clientId: String?): JSONObject? {
        val network = networkName.trim()
        if (network.isEmpty()) return null
        val payload = JSONObject().put("network_name", network)
        clientId?.let { raw ->
            val client = raw.trim()
            if (client.isEmpty()) return null
            payload.put("client_id", client)
        }
        return JSONObject().put("command", name).put("payload", payload)
    }
}
