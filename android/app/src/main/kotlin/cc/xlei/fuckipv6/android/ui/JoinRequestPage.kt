package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.Checkbox
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
import cc.xlei.fuckipv6.android.bridge.JoinRequestSurface
import cc.xlei.fuckipv6.android.bridge.JoinRequestView
import cc.xlei.fuckipv6.android.contract.SharedContracts
import java.util.Locale

/**
 * Pending join applications.
 *
 * Reviewers only see what the node published: the applicant's public profile,
 * state and revision. Approving or rejecting carries that revision, so a stale
 * decision is refused by the Coordinator instead of silently overriding a newer
 * one. A missing list is shown as unread, never as "no applicants".
 */
@Composable
fun JoinRequestListContent(
    contracts: SharedContracts,
    locale: Locale,
    requests: List<JoinRequestView>?,
    onReview: ((String, JoinRequestView) -> Unit)?,
    onBatchApprove: ((List<JoinRequestView>) -> Unit)? = null,
) {
    var pendingReview by remember { mutableStateOf<Pair<String, JoinRequestView>?>(null) }
    var selectedIds by remember(requests) { mutableStateOf(setOf<String>()) }
    var confirmBatch by remember { mutableStateOf(false) }
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .testTag("join-requests"),
    ) {
        Column(
            modifier = Modifier.padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "join.section.title"),
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                text = contracts.i18n.translate(locale, "join.section.detail"),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            when {
                requests == null -> Text(
                    modifier = Modifier.testTag("join-unobserved"),
                    text = contracts.i18n.translate(locale, "join.error.unobserved"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                requests.isEmpty() -> Text(
                    modifier = Modifier.testTag("join-empty"),
                    text = contracts.i18n.translate(locale, "common.no_data"),
                    style = MaterialTheme.typography.bodyMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                else -> requests.forEach { request ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("join-${request.requestId}"),
                        verticalAlignment = Alignment.CenterVertically,
                        horizontalArrangement = Arrangement.SpaceBetween,
                    ) {
                        if (onBatchApprove != null && request.isPending) {
                            Checkbox(
                                modifier = Modifier.testTag("join-select-${request.requestId}"),
                                checked = request.requestId in selectedIds,
                                onCheckedChange = { checked ->
                                    selectedIds = if (checked) {
                                        selectedIds + request.requestId
                                    } else {
                                        selectedIds - request.requestId
                                    }
                                },
                            )
                        }
                        Text(
                            text = request.applicantDisplayName
                                ?: request.applicantNodeId
                                ?: request.requestId,
                            style = MaterialTheme.typography.bodyLarge,
                        )
                        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                            ReviewButton(
                                contracts = contracts,
                                locale = locale,
                                tag = "join-approve-${request.requestId}",
                                labelKey = "choice.member_lifecycle.approve",
                                enabled = onReview != null && request.isPending,
                                onClick = { pendingReview = "approve" to request },
                            )
                            ReviewButton(
                                contracts = contracts,
                                locale = locale,
                                tag = "join-reject-${request.requestId}",
                                labelKey = "choice.member_lifecycle.reject",
                                enabled = onReview != null && request.isPending,
                                onClick = { pendingReview = "reject" to request },
                            )
                            ReviewButton(
                                contracts = contracts,
                                locale = locale,
                                tag = "join-info-${request.requestId}",
                                labelKey = "join.action.request_information",
                                enabled = onReview != null && request.isPending,
                                onClick = { pendingReview = "request_information" to request },
                            )
                        }
                    }
                }
            }
            if (onBatchApprove != null && !requests.isNullOrEmpty()) {
                val selected = requests.filter { it.requestId in selectedIds && it.isPending }
                TextButton(
                    modifier = Modifier.testTag("join-batch-approve"),
                    enabled = selected.isNotEmpty(),
                    onClick = { confirmBatch = true },
                ) {
                    Text(
                        contracts.i18n.translate(locale, "choice.member_lifecycle.approve") +
                            " (${selected.size})",
                    )
                }
            }
        }
    }

    if (confirmBatch) {
        val selected = requests.orEmpty().filter {
            it.requestId in selectedIds && it.isPending
        }
        AlertDialog(
            onDismissRequest = { confirmBatch = false },
            title = {
                Text(contracts.i18n.translate(locale, "confirm.change_member_lifecycle.title"))
            },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("${selected.size}")
                    Text(
                        contracts.i18n.translate(locale, "confirm.change_member_lifecycle.consequence"),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("join-batch-confirm"),
                    onClick = {
                        confirmBatch = false
                        selectedIds = emptySet()
                        onBatchApprove?.invoke(selected)
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "common.confirm_action".replace("{action}", "")))
                }
            },
            dismissButton = {
                TextButton(onClick = { confirmBatch = false }) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }

    pendingReview?.let { (action, request) ->
        AlertDialog(
            onDismissRequest = { pendingReview = null },
            title = {
                Text(contracts.i18n.translate(locale, "confirm.change_member_lifecycle.title"))
            },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(request.applicantDisplayName ?: request.applicantNodeId ?: request.requestId)
                    Text(
                        contracts.i18n.translate(locale, "confirm.change_member_lifecycle.consequence"),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("join-confirm-${request.requestId}-$action"),
                    onClick = {
                        pendingReview = null
                        onReview?.invoke(action, request)
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "choice.member_lifecycle.${
                        if (action == "request_information") "invite" else action
                    }"))
                }
            },
            dismissButton = {
                TextButton(onClick = { pendingReview = null }) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }
}

@Composable
private fun ReviewButton(
    contracts: SharedContracts,
    locale: Locale,
    tag: String,
    labelKey: String,
    enabled: Boolean,
    onClick: () -> Unit,
) {
    TextButton(
        modifier = Modifier.testTag(tag),
        enabled = enabled,
        onClick = onClick,
    ) {
        Text(
            contracts.i18n.translate(locale, labelKey),
            style = MaterialTheme.typography.labelMedium,
        )
    }
}
