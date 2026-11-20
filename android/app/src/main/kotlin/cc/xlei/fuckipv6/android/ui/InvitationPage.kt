package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.bridge.InvitationView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Invitation management.
 *
 * The list is exactly what the node returned; nothing is cached client-side. A
 * freshly created code is displayed once and never re-requested — the node does
 * not expose it again, so the UI must not pretend it can show it later.
 */
@Composable
fun InvitationListContent(
    contracts: SharedContracts,
    locale: Locale,
    invitations: List<InvitationView>?,
    issuedCode: String?,
    onCreate: (() -> Unit)?,
    onRevoke: ((InvitationView) -> Unit)?,
) {
    var pendingRevoke by remember { mutableStateOf<InvitationView?>(null) }
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("invitations"),
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
                    text = contracts.i18n.translate(locale, "invitation.section.title"),
                    style = MaterialTheme.typography.titleMedium,
                )
                TextButton(
                    modifier = Modifier.testTag("invitation-create"),
                    enabled = onCreate != null,
                    onClick = { onCreate?.invoke() },
                ) {
                    Text(contracts.i18n.translate(locale, "invitation.action.create"))
                }
            }
            Text(
                text = contracts.i18n.translate(locale, "invitation.section.detail"),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            issuedCode?.let { code ->
                Text(
                    modifier = Modifier.testTag("invitation-issued-code"),
                    text = contracts.i18n.translate(locale, "invitation.code.title") + "\n" + code,
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.primary,
                )
            }
            when {
                invitations == null -> Text(
                    modifier = Modifier.testTag("invitation-unobserved"),
                    text = contracts.i18n.translate(locale, "invitation.error.unobserved"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                invitations.isEmpty() -> Text(
                    modifier = Modifier.testTag("invitation-empty"),
                    text = contracts.i18n.translate(locale, "common.no_data"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> invitations.forEach { invitation ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("invitation-${invitation.invitationId}"),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        Column {
                            Text(
                                text = "${invitation.role ?: "unknown"} · ${invitation.state ?: "unknown"}",
                                style = MaterialTheme.typography.bodyMedium,
                            )
                            Text(
                                text = (invitation.remainingUses ?: 0).toString() + "/" +
                                    (invitation.maxUses ?: 0).toString(),
                                style = MaterialTheme.typography.labelSmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                        TextButton(
                            modifier = Modifier.testTag("invitation-revoke-${invitation.invitationId}"),
                            enabled = onRevoke != null && invitation.revokedAtUnixMs == null,
                            onClick = { pendingRevoke = invitation },
                        ) {
                            Text(contracts.i18n.translate(locale, "invitation.action.revoke"))
                        }
                    }
                }
            }
        }
    }

    pendingRevoke?.let { invitation ->
        AlertDialog(
            onDismissRequest = { pendingRevoke = null },
            title = {
                Text(contracts.i18n.translate(locale, "confirm.change_member_lifecycle.title"))
            },
            text = {
                Text(
                    contracts.i18n.translate(locale, "confirm.change_member_lifecycle.consequence"),
                    style = MaterialTheme.typography.bodySmall,
                )
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("invitation-confirm-revoke-${invitation.invitationId}"),
                    onClick = {
                        pendingRevoke = null
                        onRevoke?.invoke(invitation)
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "invitation.action.revoke"))
                }
            },
            dismissButton = {
                TextButton(onClick = { pendingRevoke = null }) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }
}
