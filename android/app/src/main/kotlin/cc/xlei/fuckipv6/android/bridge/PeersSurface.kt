package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * One peer as the node snapshot reports it.
 *
 * Endpoints are reduced to a count on purpose: the Android surface shows
 * reachability, not the peer's physical addresses, which are exactly the kind
 * of network metadata the privacy model keeps off member screens.
 */
data class PeerView(
    val peerId: String,
    val networkName: String,
    val virtualIpv6: String?,
    val virtualIpv4: String?,
    val online: Boolean,
    val p2pActive: Boolean,
    val p2pHealthy: Boolean,
    val rttUs: Long?,
    val lossRate: Float?,
    val endpointCount: Int,
)

data class PeersSnapshot(
    val networkName: String,
    val peers: List<PeerView>,
) {
    val onlineCount: Int
        get() = peers.count { it.online }
}

object PeersSurface {
    /** Peers of one network, in the order the node reported them. */
    fun parse(snapshot: JSONObject, networkName: String): PeersSnapshot? {
        val array = snapshot.opt("peers")?.let { raw ->
            when (raw) {
                null, JSONObject.NULL -> org.json.JSONArray()
                is org.json.JSONArray -> raw
                else -> return null
            }
        } ?: return null
        val peers = (0 until array.length()).mapNotNull { index ->
            val item = array.optJSONObject(index) ?: return@mapNotNull null
            peer(item)
        }.filter { it.networkName == networkName }
        return PeersSnapshot(networkName = networkName, peers = peers)
    }

    private fun peer(json: JSONObject): PeerView? {
        val peerId = json.stringOrNull("peer_id") ?: return null
        val networkName = json.stringOrNull("network_name") ?: return null
        return PeerView(
            peerId = peerId,
            networkName = networkName,
            virtualIpv6 = json.stringOrNull("virtual_ipv6"),
            virtualIpv4 = json.stringOrNull("virtual_ipv4"),
            online = json.optBoolean("online", false),
            p2pActive = json.optBoolean("p2p_active", false),
            p2pHealthy = json.optBoolean("p2p_healthy", false),
            rttUs = json.longOrNull("rtt_us"),
            lossRate = json.optDoubleOrNull("loss_rate")?.toFloat(),
            endpointCount = json.optJSONArray("endpoints")?.length() ?: 0,
        )
    }
}

internal fun JSONObject.optDoubleOrNull(key: String): Double? {
    if (!has(key) || isNull(key)) return null
    return when (val value = opt(key)) {
        is Number -> value.toDouble()
        is String -> value.toDoubleOrNull()
        else -> null
    }
}
