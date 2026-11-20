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
import cc.xlei.fuckipv6.android.bridge.IdentityView
import cc.xlei.fuckipv6.android.bridge.LogsFeed
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Node logs.
 *
 * The snapshot only carries log lines for the node owner, and ownership is not
 * exposed as a field. An empty feed therefore renders "logs are owner-only and
 * this snapshot carried none" — never "the node is quiet".
 */
@Composable
fun LogsContent(
    contracts: SharedContracts,
    locale: Locale,
    feed: LogsFeed?,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("logs"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "nav.logs"),
                style = MaterialTheme.typography.titleMedium,
            )
            when {
                feed == null -> Text(
                    modifier = Modifier.testTag("logs-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                !feed.hasLines -> Text(
                    modifier = Modifier.testTag("logs-unavailable"),
                    text = contracts.i18n.translate(locale, "logs.unavailable.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> feed.lines.forEachIndexed { index, line ->
                    Text(
                        modifier = Modifier.testTag("log-line-$index"),
                        text = "${line.level.uppercase()} ${line.target} · ${line.message}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

/**
 * The node's own identity.
 *
 * A hidden display name or fingerprint is shown as "hidden by privacy", never
 * as "unknown": the node always has both, the viewer just may not see them.
 */
@Composable
fun IdentityContent(
    contracts: SharedContracts,
    locale: Locale,
    identity: IdentityView?,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("identity"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "nav.identity"),
                style = MaterialTheme.typography.titleMedium,
            )
            if (identity == null) {
                Text(
                    modifier = Modifier.testTag("identity-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                Text(
                    modifier = Modifier.testTag("identity-node-id"),
                    text = identity.nodeId,
                    style = MaterialTheme.typography.bodyLarge,
                )
                Text(
                    modifier = Modifier.testTag("identity-display-name"),
                    text = identity.displayName
                        ?: contracts.i18n.translate(locale, "identity.hidden"),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    modifier = Modifier.testTag("identity-fingerprint"),
                    text = identity.fingerprint
                        ?: contracts.i18n.translate(locale, "identity.hidden"),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}
