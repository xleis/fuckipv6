package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.DirectoryEntry
import cc.xlei.fuckipv6.android.bridge.FileTarget
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/** Callbacks the File browser needs; keeps the shell signature small. */
data class FileBrowseActions(
    val selectTarget: (FileTarget) -> Unit,
    val openEntry: (DirectoryEntry) -> Unit,
    val up: () -> Unit,
    val loadMore: () -> Unit,
    val refresh: () -> Unit,
)

/**
 * Read-only File browser for the Android surface.
 *
 * It renders exactly what the node returned: the targets, the current portable
 * path and its entries. It offers download/upload only through callbacks the
 * host supplies, and it never fabricates an entry, a size or a "root" that the
 * node did not report.
 */
@Composable
fun FileBrowseContent(
    contracts: SharedContracts,
    locale: Locale,
    state: FileWorkspaceState,
    onSelectTarget: (FileTarget) -> Unit,
    onOpenEntry: (DirectoryEntry) -> Unit,
    onUp: () -> Unit,
    onLoadMore: () -> Unit,
    onRefresh: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Card(
            modifier = Modifier
                .fillMaxWidth()
                .testTag("file-browse-targets"),
        ) {
            Column(
                modifier = Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(locale, "file.page.target.title"),
                    style = MaterialTheme.typography.titleMedium,
                )
                if (state.targets.isEmpty()) {
                    Text(
                        text = contracts.i18n.translate(locale, "file.page.no_targets.detail"),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                    state.targets.forEach { target ->
                        val selected = state.selected?.let {
                            it.peer == target.peer && it.shareId == target.shareId
                        } == true
                        TextButton(
                            modifier = Modifier.testTag("file-target-${target.peer}-${target.shareId}"),
                            onClick = { onSelectTarget(target) },
                        ) {
                            Text(
                                text = if (selected) {
                                    contracts.i18n.translate(locale, "file.page.target.option")
                                        .replace("{network}", target.networkName)
                                        .replace("{peer}", target.peer)
                                } else {
                                    "${target.networkName} · ${target.peer}"
                                },
                            )
                        }
                    }
                }
                TextButton(
                    modifier = Modifier.testTag("file-browse-refresh"),
                    onClick = onRefresh,
                ) {
                    Text(contracts.i18n.translate(locale, "file.page.refresh"))
                }
            }
        }

        Card(
            modifier = Modifier
                .fillMaxWidth()
                .testTag("file-browse-directory"),
        ) {
            Column(
                modifier = Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.SpaceBetween,
                ) {
                    Text(
                        text = contracts.i18n.translate(locale, "file.page.directory.title"),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    TextButton(
                        modifier = Modifier.testTag("file-browse-up"),
                        enabled = state.canGoUp,
                        onClick = onUp,
                    ) {
                        Text(contracts.i18n.translate(locale, "file.page.up"))
                    }
                }
                Text(
                    modifier = Modifier.testTag("file-browse-path"),
                    text = if (SharedPathNav.isRoot(state.path)) {
                        contracts.i18n.translate(locale, "file.page.root")
                    } else {
                        state.path
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                when {
                    state.loading && state.entries.isEmpty() -> Text(
                        text = contracts.i18n.translate(locale, "file.page.directory.loading"),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    state.errorKey != null -> Text(
                        modifier = Modifier.testTag("file-browse-error"),
                        text = contracts.i18n.translate(locale, state.errorKey),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.error,
                    )
                }
                if (!state.loading && state.entries.isEmpty() && state.errorKey == null) {
                    Text(
                        text = contracts.i18n.translate(locale, "file.page.directory.empty"),
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                state.entries.forEach { entry ->
                    val name = entry.path.substringAfterLast('/')
                    TextButton(
                        modifier = Modifier.testTag("file-entry-${entry.path}"),
                        onClick = { onOpenEntry(entry) },
                    ) {
                        Text(
                            text = if (entry.isDirectory) {
                                "📁 $name"
                            } else {
                                "$name · ${entry.size} B"
                            },
                        )
                    }
                }
                if (state.truncated) {
                    Text(
                        text = contracts.i18n.translate(locale, "file.page.directory.truncated"),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (state.canLoadMore) {
                    TextButton(
                        modifier = Modifier.testTag("file-browse-load-more"),
                        onClick = onLoadMore,
                    ) {
                        Text(contracts.i18n.translate(locale, "file.page.load_more"))
                    }
                }
            }
        }
    }
}
/**
 * Transfer list summary. It shows only what the node reported: direction,
 * state, progress and the node's stable error code. Bytes are never estimated.
 */
@Composable
fun FileTransferSummary(
    contracts: SharedContracts,
    locale: Locale,
    state: FileTransferState,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("file-transfers"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "file.page.download"),
                style = MaterialTheme.typography.titleMedium,
            )
            if (state.transfers.isEmpty()) {
                Text(
                    text = contracts.i18n.translate(locale, "common.no_data"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            state.transfers.forEach { transfer ->
                Text(
                    modifier = Modifier.testTag("file-transfer-${transfer.transferId}"),
                    text = buildString {
                        append(transfer.direction ?: "transfer")
                        append(" · ")
                        append(transfer.state ?: "unknown")
                        append(" · ")
                        append("${transfer.transferredBytes}/${transfer.totalBytes} B")
                        transfer.errorCode?.let { append(" · $it") }
                    },
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            state.errorKey?.let { key ->
                Text(
                    modifier = Modifier.testTag("file-transfers-error"),
                    text = contracts.i18n.translate(locale, key),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.error,
                )
            }
        }
    }
}
