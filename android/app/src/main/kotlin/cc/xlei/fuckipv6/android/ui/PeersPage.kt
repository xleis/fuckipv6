package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.PeersSnapshot
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Peers of the selected network.
 *
 * Only reachability is shown: online/p2p flags, the virtual addresses the node
 * assigned, measured RTT and loss. Physical endpoints are intentionally reduced
 * to a count — member screens must not expose a peer's real addresses.
 */
@Composable
fun PeersContent(
    contracts: SharedContracts,
    locale: Locale,
    peers: PeersSnapshot?,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("peers"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(locale, "nav.peers"),
                    style = MaterialTheme.typography.titleMedium,
                )
                peers?.let { snapshot ->
                    Text(
                        modifier = Modifier.testTag("peers-online-count"),
                        text = "${snapshot.onlineCount}/${snapshot.peers.size}",
                        style = MaterialTheme.typography.labelMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            when {
                peers == null -> Text(
                    modifier = Modifier.testTag("peers-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                peers.peers.isEmpty() -> Text(
                    modifier = Modifier.testTag("peers-empty"),
                    text = contracts.i18n.translate(locale, "common.no_data"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> peers.peers.forEach { peer ->
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("peer-${peer.peerId}"),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        Text(peer.peerId, style = MaterialTheme.typography.bodyLarge)
                        Text(
                            text = buildString {
                                append(if (peer.online) "online" else "offline")
                                append(" · ")
                                append(
                                    when {
                                        peer.p2pHealthy -> "p2p healthy"
                                        peer.p2pActive -> "p2p active"
                                        else -> "p2p idle"
                                    },
                                )
                                peer.virtualIpv6?.let { append(" · $it") }
                                peer.rttUs?.let { append(" · ${it / 1000} ms") }
                            },
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
        }
    }
}
