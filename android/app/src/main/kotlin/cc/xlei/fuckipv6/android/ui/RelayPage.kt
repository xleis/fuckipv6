package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
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
import cc.xlei.fuckipv6.android.contract.SharedContracts
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkRole
import cc.xlei.fuckipv6.contract.NodeContractV1.NodePermission
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayGovernanceAuditSnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.RelayHealthSnapshot
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

data class RelayPageData(
    val networkName: String,
    val relayEnabled: Boolean,
    val health: RelayHealthSnapshot?,
    val governance: RelayGovernanceAuditSnapshot?,
    val grantedPermissions: Set<NodePermission>,
    val backendAvailable: Boolean,
    val onSetRelayRole: ((Boolean) -> Unit)? = null,
) {
    companion object {
        fun fromSnapshot(
            snapshot: NodeSnapshot,
            selectedNetworkName: String,
            backendAvailable: Boolean,
            onSetRelayRole: ((Boolean) -> Unit)? = null,
        ): RelayPageData? {
            if (selectedNetworkName.isBlank()) return null
            val network = snapshot.networks.orEmpty().firstOrNull {
                it.name == selectedNetworkName
            } ?: return null
            val service = snapshot.embedded_services.orEmpty().firstOrNull {
                it.network_name == selectedNetworkName
            }
            return RelayPageData(
                networkName = network.name,
                relayEnabled = network.roles.orEmpty().contains(NodeNetworkRole.RELAY),
                health = service?.relay_health,
                governance = service?.relay_governance,
                grantedPermissions = snapshot.permissions?.permissions.orEmpty().toSet(),
                backendAvailable = backendAvailable,
                onSetRelayRole = onSetRelayRole,
            )
        }
    }
}

@Composable
fun RelayPageContent(
    contracts: SharedContracts,
    locale: Locale,
    data: RelayPageData,
) {
    val actionDecision = RelayActionPolicy.evaluate(
        backendAvailable = data.backendAvailable && data.onSetRelayRole != null,
        grantedPermissions = data.grantedPermissions,
        selectedNetwork = data.networkName,
        requestedNetwork = data.networkName,
    )
    val health = data.health?.let(RelayHealthProjection::from)
    val governance = data.governance?.let(RelayGovernanceProjection::from)
    var requestedEnabled by remember(data.networkName, data.relayEnabled) {
        mutableStateOf(!data.relayEnabled)
    }
    var showConfirmation by remember { mutableStateOf(false) }

    Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
        Card(modifier = Modifier.fillMaxWidth()) {
            Column(
                modifier = Modifier.padding(20.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(
                        locale,
                        health?.statusKey ?: "relay.health.unknown",
                    ),
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    text = data.networkName,
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (health == null) {
                    Text(
                        modifier = Modifier.testTag("relay-no-health"),
                        text = contracts.i18n.translate(locale, "relay.status.no_health"),
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                    RelayHealthBody(contracts, locale, health)
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
                    text = contracts.i18n.translate(locale, "nav.relay"),
                    style = MaterialTheme.typography.titleMedium,
                )
                RelayDetail(
                    label = contracts.i18n.translate(locale, "field.enabled"),
                    value = contracts.i18n.translate(
                        locale,
                        if (data.relayEnabled) "common.enabled" else "common.disabled",
                    ),
                    modifier = Modifier.testTag("relay-role-current"),
                )
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(contracts.i18n.translate(locale, "field.enabled"))
                    Spacer(modifier = Modifier.weight(1f))
                    Switch(
                        modifier = Modifier.testTag("relay-role-enabled"),
                        checked = requestedEnabled,
                        enabled = actionDecision.canSubmit,
                        onCheckedChange = { requestedEnabled = it },
                    )
                }
                Button(
                    modifier = Modifier.testTag("relay-role-action"),
                    enabled = actionDecision.canSubmit,
                    onClick = { showConfirmation = true },
                ) {
                    Text(contracts.i18n.translate(locale, "action.set_relay_role"))
                }
                actionDecision.reasonKey?.let { reasonKey ->
                    Text(
                        modifier = Modifier.testTag("relay-role-blocked-reason"),
                        text = contracts.i18n.translate(locale, reasonKey),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }

        if (governance != null) {
            RelayGovernanceBody(contracts, locale, governance)
        }
    }

    if (showConfirmation) {
        AlertDialog(
            onDismissRequest = { showConfirmation = false },
            title = {
                Text(contracts.i18n.translate(locale, "confirm.set_relay_role.title"))
            },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(contracts.i18n.translate(locale, "confirm.set_relay_role.consequence"))
                    RelayDetail(
                        label = contracts.i18n.translate(locale, "field.enabled"),
                        value = contracts.i18n.translate(
                            locale,
                            if (requestedEnabled) "common.enabled" else "common.disabled",
                        ),
                    )
                }
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("relay-role-confirm"),
                    onClick = {
                        showConfirmation = false
                        data.onSetRelayRole?.invoke(requestedEnabled)
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "action.set_relay_role"))
                }
            },
            dismissButton = {
                TextButton(
                    modifier = Modifier.testTag("relay-role-cancel"),
                    onClick = { showConfirmation = false },
                ) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }
}

@Composable
private fun RelayHealthBody(
    contracts: SharedContracts,
    locale: Locale,
    health: RelayHealthProjection,
) {
    RelayDetail(
        label = contracts.i18n.translate(locale, "relay.lifecycle"),
        value = contracts.i18n.translate(locale, health.lifecycleKey),
    )
    RelayDetail(
        label = contracts.i18n.translate(locale, "relay.listener"),
        value = contracts.i18n.translate(
            locale,
            if (health.listenerBound) "relay.listener.bound" else "relay.listener.unbound",
        ),
        modifier = Modifier.testTag("relay-listener"),
    )
    RelayDetail(
        label = contracts.i18n.translate(locale, "relay.reachability"),
        value = contracts.i18n.translate(locale, health.reachabilityKey),
    )
    RelayDetail(
        label = contracts.i18n.translate(locale, "relay.forwarding"),
        value = contracts.i18n.translate(
            locale,
            if (health.forwardingObserved) {
                "relay.forwarding.observed"
            } else {
                "relay.forwarding.not_observed"
            },
        ),
    )
    RelayDetail(
        label = contracts.i18n.translate(locale, "relay.authorization"),
        value = contracts.i18n.translate(
            locale,
            if (health.authorizationValid) {
                "relay.authorization.valid"
            } else {
                "relay.authorization.invalid"
            },
        ),
    )
    RelayDetail(
        label = contracts.i18n.translate(locale, "relay.quota"),
        value = contracts.i18n.translate(
            locale,
            if (health.quotaLimited) "relay.quota.limited" else "relay.quota.available",
        ),
    )
    health.observedAtUnixMs?.let { observedAt ->
        RelayDetail(
            label = contracts.i18n.translate(locale, "relay.observed_at"),
            value = formatTimestamp(locale, observedAt),
        )
    }
    health.issueKeys.forEach { issueKey ->
        Text(
            text = contracts.i18n.translate(locale, issueKey),
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.error,
        )
    }
    if (health.listenAddr != null || health.reasonCode != null) {
        TechnicalDetails(
            contracts = contracts,
            locale = locale,
            testTagPrefix = "relay-tech-details",
        ) {
            health.listenAddr?.let { address ->
                TechValue(
                    label = contracts.i18n.translate(locale, "relay.listener"),
                    value = address,
                )
            }
            health.reasonCode?.let { reasonCode ->
                TechValue(
                    label = contracts.i18n.translate(locale, "relay.reason_code_label"),
                    value = reasonCode,
                )
            }
        }
    }
}

@Composable
private fun RelayGovernanceBody(
    contracts: SharedContracts,
    locale: Locale,
    governance: RelayGovernanceProjection,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(
            modifier = Modifier.padding(20.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            Text(
                modifier = Modifier.semantics { heading() },
                text = contracts.i18n.translate(locale, "gpui.section.relay_governance"),
                style = MaterialTheme.typography.titleMedium,
            )
            Text(
                text = contracts.i18n.translate(locale, "relay.governance.hint"),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Text(
                modifier = Modifier.testTag("relay-governance-count"),
                text = contracts.i18n.translate(
                    locale,
                    "relay.governance.bounded",
                    mapOf("shown" to governance.shown.toString(), "total" to governance.total.toString()),
                ),
                style = MaterialTheme.typography.bodySmall,
            )
            if (governance.dropped > 0) {
                Text(
                    text = contracts.i18n.translate(
                        locale,
                        "relay.governance.dropped",
                        mapOf("dropped" to governance.dropped.toString()),
                    ),
                    color = MaterialTheme.colorScheme.error,
                    style = MaterialTheme.typography.bodySmall,
                )
            }
            if (governance.events.isEmpty()) {
                Text(
                    text = contracts.i18n.translate(locale, "relay.governance.empty"),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            } else {
                governance.events.forEach { event ->
                    Text(
                        modifier = Modifier
                            .fillMaxWidth()
                            .testTag("relay-governance-event"),
                        text = contracts.i18n.translate(
                            locale,
                            "relay.governance.action.${event.action.wireName()}",
                        ),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                }
                val eventsWithReason = governance.events.mapNotNull { event ->
                    event.reason_code?.let { code -> event to code }
                }
                if (eventsWithReason.isNotEmpty()) {
                    TechnicalDetails(
                        contracts = contracts,
                        locale = locale,
                        testTagPrefix = "relay-governance-tech-details",
                    ) {
                        eventsWithReason.forEach { (event, code) ->
                            TechValue(
                                label = contracts.i18n.translate(
                                    locale,
                                    "relay.governance.action.${event.action.wireName()}",
                                ),
                                value = code,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun RelayDetail(
    label: String,
    value: String,
    modifier: Modifier = Modifier,
) {
    Column(modifier = modifier) {
        Text(
            text = label,
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        Text(text = value, style = MaterialTheme.typography.bodyMedium)
    }
}

private fun formatTimestamp(locale: Locale, unixMs: Long): String {
    val formatter = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss", locale)
        .withZone(ZoneId.systemDefault())
    return formatter.format(Instant.ofEpochMilli(unixMs))
}
