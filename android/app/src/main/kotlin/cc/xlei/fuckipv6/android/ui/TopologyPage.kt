package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.TopologyView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Virtualized topology list surface (W9-08 / W16-05).
 *
 * Uses Compose `LazyColumn` so 1,000 nodes / 5,000 connections render with
 * constant viewport composition cost on mobile devices while preserving full
 * search, node/connection switching and accessibility semantics.
 */
@Composable
fun TopologyContent(
    contracts: SharedContracts,
    locale: Locale,
    topology: TopologyView?,
) {
    var query by remember { mutableStateOf("") }
    var showLinks by remember { mutableStateOf(false) }

    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("topology"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "nav.topology"),
                style = MaterialTheme.typography.titleMedium,
            )
            if (topology == null) {
                Text(
                    modifier = Modifier.testTag("topology-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                val summaryText = buildString {
                    append("rev ")
                    append(topology.revision)
                    append(" · nodes ")
                    append(topology.nodes.size)
                    append("/")
                    append(topology.nodesTotal)
                    append(" · links ")
                    append(topology.connections.size)
                    append("/")
                    append(topology.connectionsTotal)
                    append(" · boundaries ")
                    append(topology.boundaryCount)
                }
                Text(
                    modifier = Modifier
                        .testTag("topology-summary")
                        .semantics { contentDescription = summaryText },
                    text = summaryText,
                    style = MaterialTheme.typography.bodyMedium,
                )
                if (topology.nodeTruncated) {
                    Text(
                        modifier = Modifier.testTag("topology-truncated"),
                        text = contracts.i18n.translate(locale, "file.page.directory.truncated"),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                OutlinedTextField(
                    modifier = Modifier
                        .fillMaxWidth()
                        .testTag("topology-filter"),
                    value = query,
                    onValueChange = { query = it },
                    label = {
                        Text(contracts.i18n.translate(locale, "android.topology.search_label"))
                    },
                    singleLine = true,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    FilterChip(
                        modifier = Modifier
                            .heightIn(min = 48.dp)
                            .testTag("topology-tab-nodes"),
                        selected = !showLinks,
                        onClick = { showLinks = false },
                        label = {
                            Text(
                                "${contracts.i18n.translate(locale, "android.topology.tab_nodes")} (${topology.nodes.size})",
                            )
                        },
                    )
                    FilterChip(
                        modifier = Modifier
                            .heightIn(min = 48.dp)
                            .testTag("topology-tab-links"),
                        selected = showLinks,
                        onClick = { showLinks = true },
                        label = {
                            Text(
                                "${contracts.i18n.translate(locale, "android.topology.tab_links")} (${topology.connections.size})",
                            )
                        },
                    )
                }

                val filterLower = query.trim().lowercase(Locale.ROOT)
                val filteredNodes = remember(topology.nodes, filterLower) {
                    if (filterLower.isEmpty()) {
                        topology.nodes
                    } else {
                        topology.nodes.filter { node ->
                            node.nodeId.lowercase(Locale.ROOT).contains(filterLower) ||
                                (node.displayName?.lowercase(Locale.ROOT)?.contains(filterLower) == true) ||
                                (node.health?.lowercase(Locale.ROOT)?.contains(filterLower) == true)
                        }
                    }
                }
                val filteredLinks = remember(topology.connections, filterLower) {
                    if (filterLower.isEmpty()) {
                        topology.connections
                    } else {
                        topology.connections.filter { link ->
                            link.from.lowercase(Locale.ROOT).contains(filterLower) ||
                                link.to.lowercase(Locale.ROOT).contains(filterLower) ||
                                (link.state?.lowercase(Locale.ROOT)?.contains(filterLower) == true)
                        }
                    }
                }

                LazyColumn(
                    modifier = Modifier
                        .fillMaxWidth()
                        .heightIn(min = 120.dp, max = 420.dp)
                        .testTag("topology-virtual-list"),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    if (!showLinks) {
                        items(
                            items = filteredNodes,
                            key = { node -> "n:${node.nodeId}" },
                        ) { node ->
                            val line = buildString {
                                append(if (node.isSelf) "★ " else "")
                                append(node.displayName ?: node.nodeId)
                                append(" · ")
                                append(node.health ?: "unknown")
                                append(" · nets ")
                                append(node.networkCount)
                            }
                            Text(
                                text = line,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .testTag("topology-node-${node.nodeId}")
                                    .semantics { contentDescription = line },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    } else {
                        items(
                            items = filteredLinks,
                            key = { link -> "l:${link.from}:${link.to}:${link.networkCount}" },
                        ) { link ->
                            val line = "${link.from} → ${link.to} · ${link.state ?: "unknown"}"
                            Text(
                                text = line,
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .testTag("topology-link-${link.from}-${link.to}")
                                    .semantics { contentDescription = line },
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }
        }
    }
}
