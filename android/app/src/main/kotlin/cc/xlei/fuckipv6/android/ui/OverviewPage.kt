package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.NetworkView
import cc.xlei.fuckipv6.android.bridge.PeersSnapshot
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Landing overview.
 *
 * Every line is an authoritative slice the bridge already projected: the node
 * state key, the network count, the peer summary and the File capability state.
 * Nothing here is derived locally, and an unobserved slice says so instead of
 * rendering a zero.
 */
@Composable
fun OverviewContent(
    contracts: SharedContracts,
    locale: Locale,
    state: SurfaceState,
    file: FilePageData?,
    networks: List<NetworkView>?,
    peers: PeersSnapshot?,
    selectedNetworkName: String?,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("overview"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "page.overview"),
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                modifier = Modifier.testTag("overview-state"),
                text = contracts.i18n.translate(locale, state.status.let { "state.${it.wireName}.title" }),
                style = MaterialTheme.typography.bodyLarge,
            )
            Text(
                modifier = Modifier.testTag("overview-networks"),
                text = buildString {
                    append("networks ")
                    append(networks?.size?.toString() ?: contracts.i18n.translate(locale, "peers.unobserved.detail"))
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                modifier = Modifier.testTag("overview-peers"),
                text = buildString {
                    append("peers ")
                    append(
                        peers?.let { "${it.onlineCount}/${it.peers.size}" }
                            ?: contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    )
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                modifier = Modifier.testTag("overview-files"),
                text = buildString {
                    append("files ")
                    append(
                        file?.let { contracts.i18n.translate(locale, it.stateKey) }
                            ?: contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    )
                },
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            selectedNetworkName?.let { name ->
                Text(
                    modifier = Modifier.testTag("overview-network"),
                    text = name,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}
