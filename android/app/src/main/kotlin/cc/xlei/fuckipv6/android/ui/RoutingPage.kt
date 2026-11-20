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
import cc.xlei.fuckipv6.android.bridge.RoutingSummaryView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Applied routing summary per network.
 *
 * Read-only: the surface shows what the node already applied and the server's
 * own vocabulary for the default action and target. Editing requires the typed
 * `update_routing` form, which is not mounted on Android yet; offering a button
 * that cannot submit would be a fake action.
 */
@Composable
fun RoutingContent(
    contracts: SharedContracts,
    locale: Locale,
    summaries: List<RoutingSummaryView>?,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("routing-summaries"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "nav.routing"),
                style = MaterialTheme.typography.titleMedium,
            )
            when {
                summaries == null -> Text(
                    modifier = Modifier.testTag("routing-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                summaries.isEmpty() -> Text(
                    modifier = Modifier.testTag("routing-empty"),
                    text = contracts.i18n.translate(locale, "common.no_data"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> summaries.forEach { summary ->
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("routing-${summary.networkName}"),
                        verticalArrangement = Arrangement.spacedBy(2.dp),
                    ) {
                        Text(summary.networkName, style = MaterialTheme.typography.bodyLarge)
                        Text(
                            text = buildString {
                                append(summary.defaultActionKind ?: "unknown")
                                summary.defaultActionValue?.let { append(" → $it") }
                                append(" · rules ")
                                append(summary.enabledRuleCount)
                                append(" · ")
                                append(summary.status ?: "unknown")
                                summary.lastAppliedRevision?.let { append(" · rev $it") }
                                summary.problemCode?.let { append(" · $it") }
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
