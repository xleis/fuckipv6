package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
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
import cc.xlei.fuckipv6.android.bridge.BlacklistView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Per-network client blacklist.
 *
 * The list always comes from the node. Banning and unbanning only submit the
 * command; once the node answers, the list is re-read, so a client that was
 * already banned (an idempotent `changed == false` answer) never looks like it
 * was newly banned by this device.
 */
@Composable
fun BlacklistContent(
    contracts: SharedContracts,
    locale: Locale,
    blacklist: BlacklistView?,
    onRefresh: () -> Unit,
    onBan: ((String) -> Unit)?,
    onUnban: ((String) -> Unit)?,
) {
    var clientId by remember { mutableStateOf("") }
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("blacklist"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "blacklist.section.title"),
                style = MaterialTheme.typography.titleMedium,
            )
            if (blacklist == null) {
                Text(
                    modifier = Modifier.testTag("blacklist-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                Text(
                    modifier = Modifier.testTag("blacklist-count"),
                    text = blacklist.clients.size.toString(),
                    style = MaterialTheme.typography.bodyMedium,
                )
                blacklist.clients.forEach { client ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("blacklist-client-$client"),
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        Text(client, style = MaterialTheme.typography.bodySmall)
                        TextButton(
                            modifier = Modifier.testTag("blacklist-unban-$client"),
                            enabled = onUnban != null,
                            onClick = { onUnban?.invoke(client) },
                        ) {
                            Text(contracts.i18n.translate(locale, "blacklist.action.unban"))
                        }
                    }
                }
            }
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                OutlinedTextField(
                    modifier = Modifier.testTag("blacklist-input"),
                    value = clientId,
                    onValueChange = { clientId = it },
                    label = { Text(contracts.i18n.translate(locale, "blacklist.input.label")) },
                    singleLine = true,
                )
                TextButton(
                    modifier = Modifier.testTag("blacklist-ban"),
                    enabled = onBan != null && clientId.isNotBlank(),
                    onClick = {
                        val target = clientId.trim()
                        clientId = ""
                        onBan?.invoke(target)
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "blacklist.action.ban"))
                }
                TextButton(
                    modifier = Modifier.testTag("blacklist-refresh"),
                    onClick = onRefresh,
                ) {
                    Text(contracts.i18n.translate(locale, "file.page.refresh"))
                }
            }
        }
    }
}
