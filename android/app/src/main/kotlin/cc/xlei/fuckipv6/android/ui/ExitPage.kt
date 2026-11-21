package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
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
import cc.xlei.fuckipv6.android.bridge.ExitSelectionView
import cc.xlei.fuckipv6.android.bridge.LocalExitView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Exit Node surface.
 *
 * This node's own Exit capability is shown as two separate consents (local owner
 * and network administrator) plus the stage, so a blocked node explains which
 * side is holding it. The per-network selection is the authority the node
 * applied; it is read-only because switching Exit needs the typed selection
 * command, which Android does not mount yet.
 */
@Composable
fun ExitContent(
    contracts: SharedContracts,
    locale: Locale,
    local: LocalExitView?,
    selections: List<ExitSelectionView>?,
    onSetLocalExit: ((Boolean) -> Unit)? = null,
) {
    var pendingEnable by remember { mutableStateOf<Boolean?>(null) }
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("exit-node"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "nav.exit_node"),
                style = MaterialTheme.typography.titleMedium,
            )
            if (local == null) {
                Text(
                    modifier = Modifier.testTag("exit-unobserved"),
                    text = contracts.i18n.translate(locale, "peers.unobserved.detail"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                Text(
                    modifier = Modifier.testTag("exit-consents"),
                    text = buildString {
                        append("owner=")
                        append(if (local.ownerExitEnabled) "on" else "off")
                        append(" · network=")
                        append(if (local.networkAllowed) "on" else "off")
                        append(" · ")
                        append(local.stage ?: "unknown")
                    },
                    style = MaterialTheme.typography.bodyMedium,
                )
                Text(
                    modifier = Modifier.testTag("exit-clients"),
                    text = "${local.activeClients}/${local.totalClients}",
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (onSetLocalExit != null) {
                    TextButton(
                        modifier = Modifier.testTag("exit-toggle"),
                        onClick = { pendingEnable = !local.ownerExitEnabled },
                    ) {
                        Text(
                            contracts.i18n.translate(
                                locale,
                                if (local.ownerExitEnabled) "exit.action.disable" else "exit.action.enable",
                            ),
                        )
                    }
                }
                local.problemCode?.let { code ->
                    Text(
                        modifier = Modifier.testTag("exit-problem"),
                        text = code,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
            }
            if (selections != null) {
                selections.forEach { selection ->
                    Text(
                        modifier = Modifier.testTag("exit-selection-${selection.networkId}"),
                        text = buildString {
                            append(selection.networkId)
                            append(" · ")
                            append(selection.mode ?: "unknown")
                            selection.fixedExit?.let { append(" · fixed $it") }
                            selection.selectedDefault?.let { append(" · default $it") }
                            append(" · rev ")
                            append(selection.policyRevision)
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
    pendingEnable?.let { enabled ->
        AlertDialog(
            onDismissRequest = { pendingEnable = null },
            title = {
                Text(contracts.i18n.translate(locale, "confirm.set_local_exit_capability.title"))
            },
            text = {
                Text(
                    contracts.i18n.translate(locale, "confirm.set_local_exit_capability.consequence"),
                    style = MaterialTheme.typography.bodySmall,
                )
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("exit-toggle-confirm"),
                    onClick = {
                        pendingEnable = null
                        onSetLocalExit?.invoke(enabled)
                    },
                ) {
                    Text(
                        contracts.i18n.translate(
                            locale,
                            if (enabled) "exit.action.enable" else "exit.action.disable",
                        ),
                    )
                }
            },
            dismissButton = {
                TextButton(onClick = { pendingEnable = null }) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }
}
