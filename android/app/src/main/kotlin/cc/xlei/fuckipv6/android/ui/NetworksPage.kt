package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.NetworkView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Networks this node belongs to, plus Create / Join / Discovery controls (W16-05).
 */
@Composable
fun NetworksContent(
    contracts: SharedContracts,
    locale: Locale,
    networks: List<NetworkView>?,
    onCreateNetwork: ((name: String) -> Unit)? = null,
    onJoinNetwork: ((ticket: String, name: String?) -> Unit)? = null,
    onRefreshDiscovery: (() -> Unit)? = null,
) {
    var newNetworkName by remember { mutableStateOf("") }
    var joinTicket by remember { mutableStateOf("") }
    var joinAlias by remember { mutableStateOf("") }

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
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
                                    network.assignedIpv4?.let { append(" · $it") }
                                    network.assignedIpv6?.let { append(" · $it") }
                                    append(" · " + (network.peerCount?.toString() ?: hidden(contracts, locale)))
                                },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
                if (onCreateNetwork != null || onJoinNetwork != null || onRefreshDiscovery != null) {
                    HorizontalDivider()
                    if (onCreateNetwork != null) {
                        OutlinedTextField(
                            modifier = Modifier
                                .fillMaxWidth()
                                .testTag("networks-create-name"),
                            value = newNetworkName,
                            onValueChange = { newNetworkName = it },
                            label = { Text(contracts.i18n.translate(locale, "android.networks.create")) },
                            singleLine = true,
                        )
                        Button(
                            modifier = Modifier
                                .heightIn(min = 48.dp)
                                .testTag("networks-create-submit"),
                            onClick = {
                                val trimmed = newNetworkName.trim()
                                if (trimmed.isNotEmpty()) {
                                    onCreateNetwork(trimmed)
                                    newNetworkName = ""
                                }
                            },
                        ) {
                            Text(contracts.i18n.translate(locale, "android.networks.create"))
                        }
                    }
                    if (onJoinNetwork != null) {
                        OutlinedTextField(
                            modifier = Modifier
                                .fillMaxWidth()
                                .testTag("networks-join-ticket"),
                            value = joinTicket,
                            onValueChange = { joinTicket = it },
                            label = { Text(contracts.i18n.translate(locale, "android.networks.join")) },
                            singleLine = true,
                        )
                        OutlinedTextField(
                            modifier = Modifier
                                .fillMaxWidth()
                                .testTag("networks-join-alias"),
                            value = joinAlias,
                            onValueChange = { joinAlias = it },
                            label = { Text(contracts.i18n.translate(locale, "nav.networks")) },
                            singleLine = true,
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            Button(
                                modifier = Modifier
                                    .heightIn(min = 48.dp)
                                    .testTag("networks-join-submit"),
                                onClick = {
                                    val ticket = joinTicket.trim()
                                    if (ticket.isNotEmpty()) {
                                        onJoinNetwork(ticket, joinAlias.trim().ifEmpty { null })
                                        joinTicket = ""
                                        joinAlias = ""
                                    }
                                },
                            ) {
                                Text(contracts.i18n.translate(locale, "android.networks.join"))
                            }
                            if (onRefreshDiscovery != null) {
                                OutlinedButton(
                                    modifier = Modifier
                                        .heightIn(min = 48.dp)
                                        .testTag("networks-discover-refresh"),
                                    onClick = onRefreshDiscovery,
                                ) {
                                    Text(contracts.i18n.translate(locale, "android.networks.discover"))
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun hidden(contracts: SharedContracts, locale: Locale): String =
    contracts.i18n.translate(locale, "identity.hidden")
