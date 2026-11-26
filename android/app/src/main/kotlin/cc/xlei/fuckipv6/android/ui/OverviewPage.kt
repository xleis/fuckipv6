package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.TextButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.NetworkView
import cc.xlei.fuckipv6.android.bridge.PeersSnapshot
import cc.xlei.fuckipv6.android.service.TunnelControl
import cc.xlei.fuckipv6.android.service.TunnelState
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
    tunnelControl: TunnelControl? = null,
    tunnelState: TunnelState? = null,
    onTunnelControl: ((TunnelControl) -> Unit)? = null,
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
            if (tunnelControl != null) {
                Text(
                    modifier = Modifier.testTag("overview-tunnel"),
                    text = buildString {
                        append("tunnel ")
                        append(
                            when (val state = tunnelState) {
                                null, TunnelState.Stopped -> "stopped"
                                TunnelState.Preparing -> "preparing"
                                TunnelState.Running -> "running"
                                TunnelState.Stopping -> "stopping"
                                is TunnelState.Failed -> "failed (${state.reasonKey})"
                            },
                        )
                        append(" · ")
                        append(
                            when (tunnelControl) {
                                TunnelControl.UNAVAILABLE -> "unavailable"
                                TunnelControl.REQUEST_CONSENT -> "needs consent"
                                TunnelControl.START -> "ready"
                                TunnelControl.WAITING -> "waiting"
                                TunnelControl.STOP -> "active"
                            },
                        )
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                when (tunnelControl) {
                    TunnelControl.REQUEST_CONSENT, TunnelControl.START, TunnelControl.STOP -> {
                        TextButton(
                            modifier = Modifier.testTag("overview-tunnel-action"),
                            onClick = { onTunnelControl?.invoke(tunnelControl) },
                        ) {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    if (tunnelControl == TunnelControl.STOP) {
                                        "tunnel.action.stop"
                                    } else {
                                        "tunnel.action.start"
                                    },
                                ),
                            )
                        }
                    }
                    TunnelControl.WAITING, TunnelControl.UNAVAILABLE -> Unit
                }
            }
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
