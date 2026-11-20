package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.semantics.heading
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import cc.xlei.fuckipv6.android.contract.SharedContracts
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryRequest
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryState
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryListCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryStatusCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeExecuteAdminRecoveryCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeRevokeAdminRecoveryCommand
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSubmitAdminRecoveryCommand
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

private const val ADMIN_RECOVERY_REASON_MAX = 512

@Composable
fun AdminRecoveryPageContent(
    contracts: SharedContracts,
    locale: Locale,
    data: AdminRecoveryPageData,
) {
    var requestJson by rememberSaveable(data.networkName) { mutableStateOf("") }
    var requestId by rememberSaveable(data.networkName) { mutableStateOf("") }
    var revocationReason by rememberSaveable(data.networkName) { mutableStateOf("") }
    var validationKey by rememberSaveable { mutableStateOf<String?>(null) }
    var pendingSubmit by remember { mutableStateOf<AdminRecoveryRequest?>(null) }
    var showRevokeConfirmation by remember { mutableStateOf(false) }
    var pendingExecuteId by rememberSaveable { mutableStateOf<String?>(null) }
    var executeStatusBaseline by rememberSaveable { mutableStateOf<Long?>(null) }

    val normalizedRequestId = requestId.trim()
    val freshExecuteStatus = pendingExecuteId?.let { pendingId ->
        if (executeStatusBaseline == null ||
            data.statusRefreshSequence <= executeStatusBaseline!!
        ) {
            null
        } else {
            data.latestStatus?.takeIf { it.requestId == pendingId }
        }
    }
    val executeBlockedKey = freshExecuteStatus?.let {
        adminRecoveryExecuteBlockedKey(it)
    }

    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(
                modifier = Modifier.padding(20.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(locale, "admin_recovery.list_title"),
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    text = data.networkName,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (!data.backendAvailable) {
                    Text(
                        modifier = Modifier.testTag("admin-recovery-unavailable"),
                        text = contracts.i18n.translate(
                            locale,
                            "admin_recovery.error.unavailable",
                        ),
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                data.networkId?.let { networkId ->
                    TechnicalDetails(
                        contracts = contracts,
                        locale = locale,
                        testTagPrefix = "admin-recovery-network-details",
                    ) {
                        TechValue(
                            label = contracts.i18n.translate(
                                locale,
                                "admin_recovery.network_id",
                            ),
                            value = networkId,
                        )
                    }
                }
            }
        }

        data.latestStatus?.let { status ->
            AdminRecoveryStatusCard(
                contracts = contracts,
                locale = locale,
                status = status,
                refreshedAtUnixMs = data.refreshedAtUnixMs,
            )
        }

        AdminRecoveryListCard(
            contracts = contracts,
            locale = locale,
            data = data,
        )

        TechnicalDetails(
            contracts = contracts,
            locale = locale,
            titleKey = "admin_recovery.advanced_title",
            hintKey = "admin_recovery.advanced_hint",
            testTagPrefix = "admin-recovery-advanced",
        ) {

            Card(modifier = Modifier.fillMaxWidth()) {
                Column(
                    modifier = Modifier.padding(20.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(
                        modifier = Modifier.semantics { heading() },
                        text = contracts.i18n.translate(locale, "action.submit_admin_recovery"),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        text = contracts.i18n.translate(locale, "admin_recovery.request_json_help"),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    OutlinedTextField(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("admin-recovery-request-json"),
                        value = requestJson,
                        onValueChange = {
                            requestJson = it
                            validationKey = null
                        },
                        label = {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "field.admin_recovery_request_json",
                                ),
                            )
                        },
                        placeholder = {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "placeholder.admin_recovery_request_json",
                                ),
                            )
                        },
                        minLines = 6,
                        maxLines = 12,
                    )
                    Button(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("admin-recovery-submit"),
                        enabled = data.backendAvailable &&
                            data.onSubmit != null &&
                            requestJson.isNotBlank(),
                        onClick = {
                            validationKey = null
                            pendingSubmit = runCatching {
                                AdminRecoveryRequestCodec.parse(requestJson)
                            }.getOrElse {
                                validationKey =
                                    "validation.admin_recovery_request_json_invalid"
                                null
                            }
                        },
                    ) {
                        Text(contracts.i18n.translate(locale, "action.submit_admin_recovery"))
                    }
                }
            }

            Card(modifier = Modifier.fillMaxWidth()) {
                Column(
                    modifier = Modifier.padding(20.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(
                        modifier = Modifier.semantics { heading() },
                        text = contracts.i18n.translate(locale, "action.get_admin_recovery_status"),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    OutlinedTextField(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("admin-recovery-request-id"),
                        value = requestId,
                        onValueChange = {
                            requestId = it
                            validationKey = null
                            pendingExecuteId = null
                            executeStatusBaseline = null
                        },
                        singleLine = true,
                        label = {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "field.admin_recovery_request_id",
                                ),
                            )
                        },
                        placeholder = {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "placeholder.admin_recovery_request_id",
                                ),
                            )
                        },
                    )
                    Row(
                        modifier = Modifier.fillMaxWidth(),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Button(
                            modifier = Modifier
                                .weight(1f)
                                .testTag("admin-recovery-status"),
                            enabled = data.backendAvailable &&
                                data.onGetStatus != null &&
                                normalizedRequestId.isNotEmpty(),
                            onClick = {
                                if (!AdminRecoveryRequestCodec.validRequestId(normalizedRequestId)) {
                                    validationKey = "validation.admin_recovery_request_id_invalid"
                                } else {
                                    validationKey = null
                                    data.onGetStatus?.invoke(
                                        NodeAdminRecoveryStatusCommand(
                                            data.networkName,
                                            normalizedRequestId,
                                        ),
                                    )
                                }
                            },
                        ) {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "action.get_admin_recovery_status",
                                ),
                            )
                        }
                        Button(
                            modifier = Modifier
                                .weight(1f)
                                .testTag("admin-recovery-list"),
                            enabled = data.backendAvailable && data.onList != null,
                            onClick = {
                                validationKey = null
                                data.onList?.invoke(
                                    NodeAdminRecoveryListCommand(data.networkName),
                                )
                            },
                        ) {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "action.list_admin_recoveries",
                                ),
                            )
                        }
                    }

                    Button(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("admin-recovery-execute-preflight"),
                        enabled = data.backendAvailable &&
                            data.onGetStatus != null &&
                            normalizedRequestId.isNotEmpty(),
                        onClick = {
                            if (!AdminRecoveryRequestCodec.validRequestId(normalizedRequestId)) {
                                validationKey = "validation.admin_recovery_request_id_invalid"
                            } else {
                                validationKey = null
                                pendingExecuteId = normalizedRequestId
                                executeStatusBaseline = data.statusRefreshSequence
                                data.onGetStatus?.invoke(
                                    NodeAdminRecoveryStatusCommand(
                                        data.networkName,
                                        normalizedRequestId,
                                    ),
                                )
                            }
                        },
                    ) {
                        Text(
                            contracts.i18n.translate(
                                locale,
                                "action.execute_admin_recovery",
                            ),
                        )
                    }
                    if (pendingExecuteId != null && freshExecuteStatus == null) {
                        Text(
                            modifier = Modifier.testTag("admin-recovery-execute-waiting"),
                            text = contracts.i18n.translate(
                                locale,
                                "admin_recovery.execution_preflight_waiting",
                            ),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    if (freshExecuteStatus != null && executeBlockedKey != null) {
                        Text(
                            modifier = Modifier.testTag("admin-recovery-execute-blocked"),
                            text = contracts.i18n.translate(locale, executeBlockedKey),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                    }
                    if (freshExecuteStatus != null && executeBlockedKey == null) {
                        Text(
                            text = contracts.i18n.translate(
                                locale,
                                "confirm.execute_admin_recovery.consequence",
                            ),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.error,
                        )
                        Button(
                            modifier = Modifier
                                .fillMaxWidth()
                                .testTag("admin-recovery-execute-confirm"),
                            enabled = data.onExecute != null,
                            onClick = {
                                val request = pendingExecuteId ?: return@Button
                                val status = freshExecuteStatus ?: return@Button
                                if (status.requestId != request ||
                                    status.state != AdminRecoveryState.PENDING ||
                                    status.canExecute != true
                                ) {
                                    return@Button
                                }
                                pendingExecuteId = null
                                executeStatusBaseline = null
                                data.onExecute?.invoke(
                                    NodeExecuteAdminRecoveryCommand(
                                        data.networkName,
                                        request,
                                    ),
                                )
                            },
                        ) {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "action.execute_admin_recovery",
                                ),
                            )
                        }
                    }
                }
            }

            Card(modifier = Modifier.fillMaxWidth()) {
                Column(
                    modifier = Modifier.padding(20.dp),
                    verticalArrangement = Arrangement.spacedBy(10.dp),
                ) {
                    Text(
                        modifier = Modifier.semantics { heading() },
                        text = contracts.i18n.translate(locale, "action.revoke_admin_recovery"),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    OutlinedTextField(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("admin-recovery-revoke-reason"),
                        value = revocationReason,
                        onValueChange = {
                            revocationReason = it
                            validationKey = null
                        },
                        minLines = 2,
                        maxLines = 5,
                        label = {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "field.admin_recovery_reason",
                                ),
                            )
                        },
                        placeholder = {
                            Text(
                                contracts.i18n.translate(
                                    locale,
                                    "placeholder.admin_recovery_reason",
                                ),
                            )
                        },
                    )
                    Text(
                        text = contracts.i18n.translate(
                            locale,
                            "admin_recovery.reason_counter",
                            mapOf(
                                "count" to revocationReason.codePointCount(
                                    0,
                                    revocationReason.length,
                                ).toString(),
                                "max" to ADMIN_RECOVERY_REASON_MAX.toString(),
                            ),
                        ),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Button(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("admin-recovery-revoke"),
                        enabled = data.backendAvailable &&
                            data.onRevoke != null &&
                            normalizedRequestId.isNotEmpty() &&
                            revocationReason.isNotEmpty(),
                        onClick = {
                            validationKey = when {
                                !AdminRecoveryRequestCodec.validRequestId(normalizedRequestId) ->
                                    "validation.admin_recovery_request_id_invalid"
                                !AdminRecoveryRequestCodec.validRevocationReason(
                                    revocationReason,
                                ) -> "validation.admin_recovery_reason_invalid"
                                else -> null
                            }
                            showRevokeConfirmation = validationKey == null
                        },
                    ) {
                        Text(
                            contracts.i18n.translate(
                                locale,
                                "action.revoke_admin_recovery",
                            ),
                        )
                    }
                }
            }

            validationKey?.let { key ->
                Text(
                    modifier = Modifier.testTag("admin-recovery-validation-error"),
                    text = contracts.i18n.translate(locale, key),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
        }

    }

    pendingSubmit?.let { request ->
        AlertDialog(
            onDismissRequest = { pendingSubmit = null },
            title = {
                Text(
                    contracts.i18n.translate(
                        locale,
                        "confirm.submit_admin_recovery.title",
                    ),
                )
            },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text(
                        contracts.i18n.translate(
                            locale,
                            "confirm.submit_admin_recovery.consequence",
                        ),
                    )
                    AdminRecoveryDetail(
                        label = contracts.i18n.translate(
                            locale,
                            "admin_recovery.impact.owner_after",
                        ),
                        value = abbreviateId(request.proposal.target.owner)
                            ?: contracts.i18n.translate(locale, "common.not_available"),
                    )
                }
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("admin-recovery-submit-confirm"),
                    onClick = {
                        pendingSubmit = null
                        data.onSubmit?.invoke(
                            NodeSubmitAdminRecoveryCommand(
                                data.networkName,
                                request,
                            ),
                        )
                    },
                ) {
                    Text(
                        contracts.i18n.translate(
                            locale,
                            "action.submit_admin_recovery",
                        ),
                    )
                }
            },
            dismissButton = {
                TextButton(
                    onClick = { pendingSubmit = null },
                ) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }

    if (showRevokeConfirmation) {
        AlertDialog(
            onDismissRequest = { showRevokeConfirmation = false },
            title = {
                Text(
                    contracts.i18n.translate(
                        locale,
                        "confirm.revoke_admin_recovery.title",
                    ),
                )
            },
            text = {
                Text(
                    contracts.i18n.translate(
                        locale,
                        "confirm.revoke_admin_recovery.consequence",
                    ),
                )
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("admin-recovery-revoke-confirm"),
                    onClick = {
                        showRevokeConfirmation = false
                        data.onRevoke?.invoke(
                            NodeRevokeAdminRecoveryCommand(
                                data.networkName,
                                revocationReason,
                                normalizedRequestId,
                            ),
                        )
                    },
                ) {
                    Text(
                        contracts.i18n.translate(
                            locale,
                            "action.revoke_admin_recovery",
                        ),
                    )
                }
            },
            dismissButton = {
                TextButton(
                    onClick = { showRevokeConfirmation = false },
                ) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }
}

@Composable
private fun AdminRecoveryListCard(
    contracts: SharedContracts,
    locale: Locale,
    data: AdminRecoveryPageData,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                horizontalArrangement = Arrangement.SpaceBetween,
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(locale, "admin_recovery.list_title"),
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    text = contracts.i18n.translate(
                        locale,
                        "admin_recovery.list_count",
                        mapOf("count" to data.recoveries.size.toString()),
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            data.loadedAtUnixMs?.let { loadedAt ->
                Text(
                    text = contracts.i18n.translate(
                        locale,
                        "admin_recovery.loaded_at",
                        mapOf(
                            "time" to formatAdminRecoveryTimestamp(locale, loadedAt),
                        ),
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            if (data.recoveries.isEmpty()) {
                Text(
                    modifier = Modifier.testTag("admin-recovery-list-empty"),
                    text = contracts.i18n.translate(locale, "admin_recovery.empty"),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                data.recoveries.forEach { status ->
                    AdminRecoveryStatusCard(
                        contracts = contracts,
                        locale = locale,
                        status = status,
                    )
                }
            }
        }
    }
}

@Composable
private fun AdminRecoveryStatusCard(
    contracts: SharedContracts,
    locale: Locale,
    status: AdminRecoveryStatusProjection,
    refreshedAtUnixMs: Long? = null,
) {
    val notAvailable = contracts.i18n.translate(locale, "common.not_available")
    val none = contracts.i18n.translate(locale, "common.none")
    val state = status.state?.let {
        contracts.i18n.translate(locale, "admin_recovery.state.${it.wireName()}")
    } ?: notAvailable

    Card(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier
                .padding(16.dp)
                .testTag("admin-recovery-status-card"),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(
                    locale,
                    "admin_recovery.status_title",
                ),
                style = MaterialTheme.typography.titleSmall,
            )
            Text(text = state, style = MaterialTheme.typography.titleMedium)
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.impact.owner_before"),
                status.impact?.ownerBefore ?: notAvailable,
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.impact.owner_after"),
                status.impact?.ownerAfter ?: notAvailable,
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(
                    locale,
                    "admin_recovery.impact.administrators_added",
                ),
                adminRecoveryListText(
                    status.impact?.administratorsAdded,
                    notAvailable,
                    none,
                ),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(
                    locale,
                    "admin_recovery.impact.administrators_removed",
                ),
                adminRecoveryListText(
                    status.impact?.administratorsRemoved,
                    notAvailable,
                    none,
                ),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.impact.members_added"),
                adminRecoveryListText(
                    status.impact?.membersAdded,
                    notAvailable,
                    none,
                ),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.impact.members_removed"),
                adminRecoveryListText(
                    status.impact?.membersRemoved,
                    notAvailable,
                    none,
                ),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.signatures"),
                contracts.i18n.translate(
                    locale,
                    "admin_recovery.signature_count",
                    mapOf(
                        "count" to (status.signatureCount?.toString() ?: notAvailable),
                        "required" to (status.requiredSignatures?.toString() ?: notAvailable),
                    ),
                ),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.issued_at"),
                status.issuedAtUnixMs?.let {
                    formatAdminRecoveryTimestamp(locale, it)
                } ?: notAvailable,
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.expires_at"),
                status.expiresAtUnixMs?.let {
                    formatAdminRecoveryTimestamp(locale, it)
                } ?: notAvailable,
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(
                    locale,
                    "admin_recovery.revocation.old_sessions",
                ),
                adminRecoveryFlag(contracts, locale, status.revocation?.oldSessions),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(
                    locale,
                    "admin_recovery.revocation.pending_invitations",
                ),
                adminRecoveryFlag(
                    contracts,
                    locale,
                    status.revocation?.pendingInvitations,
                ),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(
                    locale,
                    "admin_recovery.revocation.unconfirmed_writes",
                ),
                adminRecoveryFlag(
                    contracts,
                    locale,
                    status.revocation?.unconfirmedWrites,
                ),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.can_execute"),
                adminRecoveryFlag(contracts, locale, status.canExecute),
            )
            AdminRecoveryDetail(
                contracts.i18n.translate(locale, "admin_recovery.denial"),
                status.denialKey?.let { contracts.i18n.translate(locale, it) } ?: none,
            )
            TechnicalDetails(
                contracts = contracts,
                locale = locale,
                testTagPrefix = "admin-recovery-status-details",
            ) {
                TechValue(
                    label = contracts.i18n.translate(
                        locale,
                        "field.admin_recovery_request_id",
                    ),
                    value = status.requestId ?: notAvailable,
                )
                TechValue(
                    label = contracts.i18n.translate(locale, "admin_recovery.network_id"),
                    value = status.networkId ?: notAvailable,
                )
                TechValue(
                    label = contracts.i18n.translate(locale, "admin_recovery.initiator"),
                    value = status.initiator ?: notAvailable,
                )
                TechValue(
                    label = contracts.i18n.translate(locale, "admin_recovery.participants"),
                    value = adminRecoveryListText(status.participants, notAvailable, none),
                )
            }
            refreshedAtUnixMs?.let {
                Text(
                    text = contracts.i18n.translate(
                        locale,
                        "admin_recovery.refreshed_at",
                        mapOf(
                            "time" to formatAdminRecoveryTimestamp(locale, it),
                        ),
                    ),
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
    }
}

@Composable
private fun AdminRecoveryDetail(
    label: String,
    value: String,
) {
    Column {
        Text(
            text = label,
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(text = value, style = MaterialTheme.typography.bodyMedium)
    }
}

private fun adminRecoveryListText(
    values: List<String>?,
    notAvailable: String,
    none: String,
): String = when {
    values == null -> notAvailable
    values.isEmpty() -> none
    else -> values.joinToString(" · ")
}

private fun adminRecoveryFlag(
    contracts: SharedContracts,
    locale: Locale,
    value: Boolean?,
): String = when (value) {
    null -> contracts.i18n.translate(locale, "common.not_available")
    true -> contracts.i18n.translate(locale, "common.yes")
    false -> contracts.i18n.translate(locale, "common.no")
}

private fun adminRecoveryExecuteBlockedKey(
    status: AdminRecoveryStatusProjection,
): String? = when {
    status.state == AdminRecoveryState.EXECUTED -> "admin_recovery.error.already_executed"
    status.state == AdminRecoveryState.REVOKED -> "admin_recovery.error.revoked"
    status.state == AdminRecoveryState.EXPIRED -> "admin_recovery.error.expired"
    status.state != AdminRecoveryState.PENDING -> "admin_recovery.error.not_executable"
    status.canExecute != true ->
        status.denialKey ?: "admin_recovery.error.not_executable"
    else -> null
}

private fun formatAdminRecoveryTimestamp(locale: Locale, unixMs: Long): String {
    val formatter = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss", locale)
        .withZone(ZoneId.systemDefault())
    return formatter.format(Instant.ofEpochMilli(unixMs))
}
