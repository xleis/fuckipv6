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
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Networks this node belongs to.
 *
 * A field the privacy policy withholds renders the shared "hidden by the policy"
 * copy. It must never render as zero/blank: "no peers" and "peer count hidden"
 * are different facts, and only the node knows which one is true.
 */
@Composable
fun NetworksContent(
    contracts: SharedContracts,
    locale: Locale,
    networks: List<NetworkView>?,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("networks"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "nav.networks"),
                style = MaterialTheme.typography.titleMedium,
            )
            when {
                networks == null -> Text(
                    modifier = Modifier.testTag("networks-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                networks.isEmpty() -> Text(
                    modifier = Modifier.testTag("networks-empty"),
                    text = contracts.i18n.translate(locale, "common.no_data"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> networks.forEach { network ->
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("network-${network.name}"),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        Text(network.name, style = MaterialTheme.typography.bodyLarge)
                        Text(
                            text = buildString {
                                append(network.status ?: hidden(contracts, locale))
                                network.roles?.let { roles ->
                                    if (roles.isNotEmpty()) append(" · " + roles.joinToString("/"))
                                }
                                network.assignedIpv6?.let { append(" · $it") }
                                append(" · " + (network.peerCount?.toString() ?: hidden(contracts, locale)))
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

@Composable
private fun hidden(contracts: SharedContracts, locale: Locale): String =
    contracts.i18n.translate(locale, "identity.hidden")
