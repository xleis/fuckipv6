package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject

/**
 * Per-network traffic counters the node reports.
 *
 * Numbers are shown as reported; the Android surface never derives rates from
 * itself (a locally computed "speed" would be a second source of truth and
 * would break as soon as the poll interval changes).
 */
data class TrafficView(
    val networkName: String,
    val tunRxPackets: Long,
    val tunRxBytes: Long,
    val tunTxPackets: Long,
    val tunTxBytes: Long,
    val p2pRxPackets: Long,
    val p2pRxBytes: Long,
    val p2pTxPackets: Long,
    val p2pTxBytes: Long,
    val errorCount: Long,
)

object TrafficSurface {
    fun parse(snapshot: JSONObject, networkName: String): TrafficView? {
        val networks = snapshot.optJSONArray("networks") ?: return null
        val network = (0 until networks.length())
            .mapNotNull { networks.optJSONObject(it) }
            .firstOrNull { it.stringOrNull("name") == networkName }
            ?: return null
        val traffic = network.optJSONObject("traffic") ?: return null
        return TrafficView(
            networkName = networkName,
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
    }
}
