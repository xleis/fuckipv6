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
import cc.xlei.fuckipv6.android.bridge.TopologyView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Topology as a bounded list, not a graph.
 *
 * The desktop surfaces draw the graph; on a phone the honest equivalent is a
 * list that says how many nodes and connections exist and whether the node
 * already truncated the topology. Drawing a partial graph would imply a shape
 * this device never received.
 */
@Composable
fun TopologyContent(
    contracts: SharedContracts,
    locale: Locale,
    topology: TopologyView?,
) {
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
                Text(
                    modifier = Modifier.testTag("topology-summary"),
                    text = buildString {
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
                    },
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
                topologyshown(topology).forEach { line ->
                    Text(
                        text = line.first,
                        modifier = Modifier.testTag(line.second),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

private fun topologyshown(topology: TopologyView): List<Pair<String, String>> =
    topology.nodes.map { node ->
        buildString {
            append(if (node.isSelf) "★ " else "")
            append(node.displayName ?: node.nodeId)
            append(" · ")
            append(node.health ?: "unknown")
            append(" · nets ")
            append(node.networkCount)
        } to "topology-node-${node.nodeId}"
    } + topology.connections.map { connection ->
        buildString {
            append(connection.from)
            append(" → ")
            append(connection.to)
            append(" · ")
            append(connection.state ?: "unknown")
        } to "topology-link-${connection.from}-${connection.to}"
    }
