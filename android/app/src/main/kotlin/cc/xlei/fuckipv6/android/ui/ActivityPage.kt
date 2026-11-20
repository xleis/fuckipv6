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
import cc.xlei.fuckipv6.android.bridge.ActivityFeed
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Recent activity for the selected network.
 *
 * Action and outcome labels are translated only when the catalog actually has
 * the key (`translateOr`); otherwise the server's stable identifier is shown.
 * Raw `translate` would print the key itself, which is exactly the kind of
 * "looks localized but isn't" defect this surface avoids.
 */
@Composable
fun ActivityContent(
    contracts: SharedContracts,
    locale: Locale,
    feed: ActivityFeed?,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("activity-feed"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "nav.activity"),
                style = MaterialTheme.typography.titleMedium,
            )
            when {
                feed == null -> Text(
                    modifier = Modifier.testTag("activity-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                feed.entries.isEmpty() -> Text(
                    modifier = Modifier.testTag("activity-empty"),
                    text = contracts.i18n.translate(locale, "common.no_data"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> feed.entries.forEach { entry ->
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("activity-${entry.auditId}"),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        Text(
                            text = contracts.i18n.translateOr(
                                locale,
                                "activity.action.${entry.action}",
                                entry.action,
                            ),
                            style = MaterialTheme.typography.bodyLarge,
                        )
                        Text(
                            text = buildString {
                                append(
                                    contracts.i18n.translateOr(
                                        locale,
                                        "activity.outcome.${entry.outcome}",
                                        entry.outcome,
                                    ),
                                )
                                append(" · ")
                                append(entry.resource)
                                entry.summary?.let { append(" · $it") }
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

/** Per-network traffic counters, exactly as the node reported them. */
@Composable
fun TrafficContent(
    contracts: SharedContracts,
    locale: Locale,
    traffic: cc.xlei.fuckipv6.android.bridge.TrafficView?,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("traffic-counters"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "nav.traffic"),
                style = MaterialTheme.typography.titleMedium,
            )
            if (traffic == null) {
                Text(
                    modifier = Modifier.testTag("traffic-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                TrailRow(
                    contracts = contracts,
                    locale = locale,
                    tag = "traffic-tun-rx",
                    labelKey = "traffic.row.tun_rx",
                    value = FilePageData.formatBytes(traffic.tunRxBytes),
                )
                TrailRow(
                    contracts = contracts,
                    locale = locale,
                    tag = "traffic-tun-tx",
                    labelKey = "traffic.row.tun_tx",
                    value = FilePageData.formatBytes(traffic.tunTxBytes),
                )
                TrailRow(
                    contracts = contracts,
                    locale = locale,
                    tag = "traffic-errors",
                    labelKey = "traffic.row.errors",
                    value = traffic.errorCount.toString(),
                )
            }
        }
    }
}

@Composable
private fun TrailRow(
    contracts: SharedContracts,
    locale: Locale,
    tag: String,
    labelKey: String,
    value: String,
) {
    Column(
        modifier = Modifier.fillMaxWidth(),
        verticalArrangement = Arrangement.spacedBy(1.dp),
    ) {
        Text(
            text = contracts.i18n.translate(locale, labelKey),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            modifier = Modifier.testTag(tag),
            text = value,
            style = MaterialTheme.typography.bodyLarge,
        )
    }
}
