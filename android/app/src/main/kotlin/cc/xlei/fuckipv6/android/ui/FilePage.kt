package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * File capability card for the Android surface.
 *
 * The Android app has no node bridge yet, so this page renders only the
 * authoritative capability state the node publishes. It deliberately offers no
 * browse/upload/download action: an action without an authenticated node API
 * would have to invent success, which is exactly what the File contract
 * forbids. Transfers stay on the desktop surfaces, and the note explains that
 * share roots, enablement, ACLs and quotas stay in the owner workflow.
 */
@Composable
fun FilePageContent(
    contracts: SharedContracts,
    locale: Locale,
    data: FilePageData,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("file-capability-card"),
    ) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(locale, "file.page.local_state.title"),
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    modifier = Modifier.testTag("file-capability-state"),
                    text = contracts.i18n.translate(locale, data.stateKey),
                    style = MaterialTheme.typography.labelLarge,
                    color = when (data.stateTone) {
                        "good" -> MaterialTheme.colorScheme.primary
                        "warn" -> MaterialTheme.colorScheme.tertiary
                        else -> MaterialTheme.colorScheme.onSurfaceVariant
                    },
                )
            }

            data.networkLabel?.let { network ->
                FileDetailRow(
                    contracts = contracts,
                    locale = locale,
                    testTag = "file-capability-network",
                    labelKey = "settings.row.file_network",
                    value = network,
                )
            }
            FileDetailRow(
                contracts = contracts,
                locale = locale,
                testTag = "file-capability-transfers",
                labelKey = "settings.row.file_active_transfers",
                value = data.activeTransfers.toString(),
            )
            data.formattedAvailable?.let { available ->
                FileDetailRow(
                    contracts = contracts,
                    locale = locale,
                    testTag = "file-capability-available",
                    labelKey = "file.page.local_state.available",
                    value = available,
                )
            }
            data.rootErrorCode?.let {
                Text(
                    modifier = Modifier.testTag("file-capability-root-error"),
                    text = contracts.i18n.translate(
                        locale,
                        "file.page.local_state.root_error",
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )
            }

            if (data.shareRevision != null ||
                data.accessEpoch != null ||
                data.rootErrorCode != null
            ) {
                TechnicalDetails(
                    contracts = contracts,
                    locale = locale,
                    testTagPrefix = "file-tech-details",
                ) {
                    data.shareRevision?.let { revision ->
                        TechValue(
                            label = contracts.i18n.translate(
                                locale,
                                "file.page.local_state.share_revision",
                            ),
                            value = revision.toString(),
                        )
                    }
                    data.accessEpoch?.let { epoch ->
                        TechValue(
                            label = contracts.i18n.translate(
                                locale,
                                "file.page.local_state.access_epoch",
                            ),
                            value = epoch.toString(),
                        )
                    }
                    data.rootErrorCode?.let { code ->
                        TechValue(
                            label = contracts.i18n.translate(
                                locale,
                                "file.page.transfer.error_code",
                            ),
                            value = code,
                        )
                    }
                }
            }
            Text(
                text = contracts.i18n.translate(locale, "file.page.local_state.detail"),
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                modifier = Modifier.testTag("file-capability-transfer-surface"),
                text = contracts.i18n.translate(locale, data.transferSurfaceKey),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

@Composable
private fun FileDetailRow(
    contracts: SharedContracts,
    locale: Locale,
    testTag: String,
    labelKey: String,
    value: String,
) {
    Row(
        modifier = Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.SpaceBetween,
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            modifier = Modifier.weight(1f, fill = false),
            text = contracts.i18n.translate(locale, labelKey),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            modifier = Modifier
                .padding(start = 12.dp)
                .testTag(testTag),
            text = value,
            style = MaterialTheme.typography.bodyMedium,
            maxLines = 1,
            softWrap = false,
        )
    }
}
