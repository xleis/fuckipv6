package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
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
    networkName: String? = null,
    onUpdateRouting: ((mode: String, exitNode: String?, isExitNode: Boolean) -> Unit)? = null,
) {
    var selectedMode by remember(networkName) { mutableStateOf("mesh_only") }
    var exitNode by remember(networkName) { mutableStateOf("") }
    var isExitNode by remember(networkName) { mutableStateOf(false) }
    var confirm by remember { mutableStateOf(false) }
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
            if (onUpdateRouting != null && !networkName.isNullOrBlank()) {
                Text(
                    text = contracts.i18n.translate(locale, "action.update_routing"),
                    style = MaterialTheme.typography.labelLarge,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    for (mode in cc.xlei.fuckipv6.android.bridge.RoutingSurface.MODES) {
                        FilterChip(
                            modifier = Modifier.testTag("routing-mode-$mode"),
                            selected = mode == selectedMode,
                            onClick = { selectedMode = mode },
                            label = {
                                Text(contracts.i18n.translate(locale, "choice.routing.$mode"))
                            },
                        )
                    }
                }
                OutlinedTextField(
                    modifier = Modifier
                        .fillMaxWidth()
                        .testTag("routing-exit-input"),
                    value = exitNode,
                    onValueChange = { exitNode = it },
                    label = { Text(contracts.i18n.translate(locale, "nav.exit_node")) },
                    singleLine = true,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(
                        text = contracts.i18n.translate(locale, "action.set_local_exit_capability"),
                        style = MaterialTheme.typography.bodySmall,
                    )
                    Switch(
                        modifier = Modifier.testTag("routing-is-exit"),
                        checked = isExitNode,
                        onCheckedChange = { isExitNode = it },
                    )
                }
                TextButton(
                    modifier = Modifier.testTag("routing-submit"),
                    onClick = { confirm = true },
                ) {
                    Text(contracts.i18n.translate(locale, "action.update_routing"))
                }
            }
        }
    }
    if (confirm) {
        AlertDialog(
            onDismissRequest = { confirm = false },
            title = {
                Text(contracts.i18n.translate(locale, "confirm.update_routing.title"))
            },
            text = {
                Text(
                    contracts.i18n.translate(locale, "confirm.update_routing.consequence"),
                    style = MaterialTheme.typography.bodySmall,
                )
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("routing-confirm"),
                    onClick = {
                        confirm = false
                        onUpdateRouting?.invoke(
                            selectedMode,
                            exitNode.trim().ifEmpty { null },
                            isExitNode,
                        )
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "action.update_routing"))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirm = false }) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }
}
