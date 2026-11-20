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
import cc.xlei.fuckipv6.android.bridge.MemberDirectoryView
import cc.xlei.fuckipv6.android.bridge.MemberView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Member roster section.
 *
 * It renders only what the node's directory returned: identity, the display
 * name the member chose to publish, role, connection and lifecycle state, plus
 * the actions the current viewer may take. No member, action or presence is
 * inferred locally; an unread directory says so.
 */
@Composable
fun MemberListContent(
    contracts: SharedContracts,
    locale: Locale,
    directory: MemberDirectoryView?,
    onRefresh: () -> Unit,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("member-directory"),
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
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(locale, "member.section.title"),
                    style = MaterialTheme.typography.titleMedium,
                )
                TextButton(
                    modifier = Modifier.testTag("member-refresh"),
                    onClick = onRefresh,
                ) {
                    Text(contracts.i18n.translate(locale, "file.page.refresh"))
                }
            }
            Text(
                text = contracts.i18n.translate(locale, "member.section.detail"),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            if (directory == null) {
                Text(
                    modifier = Modifier.testTag("member-unobserved"),
                    text = contracts.i18n.translate(locale, "member.error.unobserved"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else if (directory.members.isEmpty()) {
                Text(
                    modifier = Modifier.testTag("member-empty"),
                    text = contracts.i18n.translate(locale, "common.no_data"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                directory.members.forEach { member ->
                    MemberRow(contracts = contracts, locale = locale, member = member)
                }
            }
        }
    }
}

@Composable
private fun MemberRow(
    contracts: SharedContracts,
    locale: Locale,
    member: MemberView,
) {
    val label = member.displayName ?: member.nodeId
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("member-${member.nodeId}"),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(text = label, style = MaterialTheme.typography.bodyLarge)
        Text(
            text = "${member.role ?: "unknown"} · ${member.status ?: "unknown"}",
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(
            text = contracts.i18n.translate(locale, "member.row.lifecycle") +
                ": " + (member.lifecycleState ?: "unknown") +
                " · " + contracts.i18n.translate(locale, "member.row.actions") +
                ": " + member.availableActions.size,
            style = MaterialTheme.typography.labelSmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}
