package cc.xlei.fuckipv6.android.bridge

import org.json.JSONArray
import org.json.JSONObject

/**
 * One network as the snapshot exposes it to this viewer.
 *
 * Every optional field is `null` when the privacy policy hides it — not because
 * the value is missing. The UI distinguishes the two: hidden fields render the
 * shared "hidden by the privacy policy" copy, never a blank that reads as zero.
 */
data class NetworkView(
    val name: String,
    val tunName: String?,
    val status: String?,
    val assignedIpv6: String?,
    val assignedIpv4: String?,
    val listenAddr: String?,
    val uptimeSecs: Long?,
    /** `null` when roles are hidden, empty when the node reports none. */
    val roles: List<String>?,
    val peerCount: Int?,
    val traffic: TrafficView?,
)

object NetworksSurface {
    fun parse(snapshot: JSONObject): List<NetworkView>? {
        val raw = snapshot.opt("networks") ?: return null
        val array = when (raw) {
            is JSONArray -> raw
            JSONObject.NULL -> JSONArray()
            else -> return null
        }
        return (0 until array.length()).mapNotNull { index ->
            array.optJSONObject(index)?.let(::network)
        }
    }

    private fun network(json: JSONObject): NetworkView? {
        val name = json.stringOrNull("name") ?: return null
        return NetworkView(
            name = name,
            tunName = json.stringOrNull("tun_name"),
            status = json.stringOrNull("status"),
            assignedIpv6 = json.stringOrNull("assigned_ipv6"),
            assignedIpv4 = json.stringOrNull("assigned_ipv4"),
            listenAddr = json.stringOrNull("p2p_listen_addr"),
            uptimeSecs = json.longOrNull("uptime_secs"),
            roles = json.opt("roles")?.let { value ->
                when (value) {
                    is JSONArray -> (0 until value.length())
                        .mapNotNull { index -> value.optString(index, "").takeIf { it.isNotEmpty() } }
                    JSONObject.NULL -> emptyList()
                    else -> null
                }
            },
            peerCount = json.longOrNull("peer_count")?.toInt(),
            traffic = json.optJSONObject("traffic")?.let { traffic ->
                TrafficView(
                    networkName = name,
                    tunRxPackets = traffic.longOrNull("tun_rx_packets") ?: 0L,
                    tunRxBytes = traffic.longOrNull("tun_rx_bytes") ?: 0L,
                    tunTxPackets = traffic.longOrNull("tun_tx_packets") ?: 0L,
                    tunTxBytes = traffic.longOrNull("tun_tx_bytes") ?: 0L,
                    p2pRxPackets = traffic.longOrNull("p2p_rx_packets") ?: 0L,
                    p2pRxBytes = traffic.longOrNull("p2p_rx_bytes") ?: 0L,
                    p2pTxPackets = traffic.longOrNull("p2p_tx_packets") ?: 0L,
                    p2pTxBytes = traffic.longOrNull("p2p_tx_bytes") ?: 0L,
                    errorCount = traffic.longOrNull("error_count") ?: 0L,
                )
            },
        )
    }
}
