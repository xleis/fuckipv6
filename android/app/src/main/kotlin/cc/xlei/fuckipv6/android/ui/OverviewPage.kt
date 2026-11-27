package cc.xlei.fuckipv6.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import cc.xlei.fuckipv6.android.bridge.NetworkView
import cc.xlei.fuckipv6.android.bridge.PeersSnapshot
import cc.xlei.fuckipv6.android.contract.SharedContracts
import cc.xlei.fuckipv6.android.platform.DeviceEnvironmentSnapshot
import cc.xlei.fuckipv6.android.platform.DevicePolicyDecision
import cc.xlei.fuckipv6.android.service.EmbeddedNodeEndpoint
import cc.xlei.fuckipv6.android.service.NativeNodeBridge
import cc.xlei.fuckipv6.android.service.TunnelControl
import cc.xlei.fuckipv6.android.service.TunnelState
import java.util.Locale

/**
 * Landing overview with VPN pre-consent disclosure (W16-02 #7), embedded node
 * telemetry, and battery/Doze/metered health card (W16-06).
 */
@Composable
fun OverviewContent(
    contracts: SharedContracts,
    locale: Locale,
    state: SurfaceState,
    file: FilePageData?,
    networks: List<NetworkView>?,
    peers: PeersSnapshot?,
    selectedNetworkName: String?,
    tunnelControl: TunnelControl? = null,
    tunnelState: TunnelState? = null,
    onTunnelControl: ((TunnelControl) -> Unit)? = null,
    onStopDataPlaneOnly: (() -> Unit)? = null,
    embeddedEndpoint: EmbeddedNodeEndpoint? = null,
    nativeTelemetry: NativeNodeBridge.NativeTelemetry? = null,
    onStartEmbeddedNode: (() -> Unit)? = null,
    deviceEnv: DeviceEnvironmentSnapshot? = null,
    devicePolicy: DevicePolicyDecision? = null,
    onOpenBatterySettings: (() -> Unit)? = null,
) {
    var showVpnConsentExplain by remember { mutableStateOf(false) }

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Card(
            modifier = Modifier
                .fillMaxWidth()
                .testTag("overview"),
        ) {
            Column(
                modifier = Modifier.padding(16.dp),
                verticalArrangement = Arrangement.spacedBy(8.dp),
            ) {
                Text(
                    modifier = Modifier.semantics { heading() },
                    text = contracts.i18n.translate(locale, "page.overview"),
                    style = MaterialTheme.typography.titleMedium,
                )
                Text(
                    modifier = Modifier.testTag("overview-state"),
                    text = contracts.i18n.translate(locale, state.status.let { "state.${it.wireName}.title" }),
                    style = MaterialTheme.typography.bodyLarge,
                )
                Text(
                    modifier = Modifier.testTag("overview-networks"),
                    text = buildString {
                        append("networks ")
                        append(networks?.size?.toString() ?: contracts.i18n.translate(locale, "peers.unobserved.detail"))
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    modifier = Modifier.testTag("overview-peers"),
                    text = buildString {
                        append("peers ")
                        append(
                            peers?.let { "${it.onlineCount}/${it.peers.size}" }
                                ?: contracts.i18n.translate(locale, "peers.unobserved.detail"),
                        )
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                Text(
                    modifier = Modifier.testTag("overview-files"),
                    text = buildString {
                        append("files ")
                        append(
                            file?.let { contracts.i18n.translate(locale, it.stateKey) }
                                ?: contracts.i18n.translate(locale, "peers.unobserved.detail"),
                        )
                    },
                    style = MaterialTheme.typography.bodySmall,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
                if (tunnelControl != null) {
                    Text(
                        modifier = Modifier.testTag("overview-tunnel"),
                        text = buildString {
                            append("tunnel ")
                            append(
                                when (val current = tunnelState) {
                                    null, TunnelState.Stopped -> "stopped"
                                    TunnelState.Preparing -> "preparing"
                                    TunnelState.Running -> "running"
                                    TunnelState.Stopping -> "stopping"
                                    is TunnelState.Failed -> "failed (${current.reasonKey})"
                                },
                            )
                            append(" · ")
                            append(
                                when (tunnelControl) {
                                    TunnelControl.UNAVAILABLE -> "unavailable"
                                    TunnelControl.REQUEST_CONSENT -> "needs consent"
                                    TunnelControl.START -> "ready"
                                    TunnelControl.WAITING -> "waiting"
                                    TunnelControl.STOP -> "active"
                                },
                            )
                        },
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        when (tunnelControl) {
                            TunnelControl.REQUEST_CONSENT -> {
                                Button(
                                    modifier = Modifier
                                        .heightIn(min = 48.dp)
                                        .testTag("overview-tunnel-action"),
                                    onClick = { showVpnConsentExplain = true },
                                ) {
                                    Text(contracts.i18n.translate(locale, "tunnel.action.start"))
                                }
                            }
                            TunnelControl.START, TunnelControl.STOP -> {
                                TextButton(
                                    modifier = Modifier
                                        .heightIn(min = 48.dp)
                                        .testTag("overview-tunnel-action"),
                                    onClick = { onTunnelControl?.invoke(tunnelControl) },
                                ) {
                                    Text(
                                        contracts.i18n.translate(
                                            locale,
                                            if (tunnelControl == TunnelControl.STOP) {
                                                "tunnel.action.stop"
                                            } else {
                                                "tunnel.action.start"
                                            },
                                        ),
                                    )
                                }
                            }
                            TunnelControl.WAITING, TunnelControl.UNAVAILABLE -> Unit
                        }
                        if (tunnelControl == TunnelControl.STOP && onStopDataPlaneOnly != null) {
                            OutlinedButton(
                                modifier = Modifier
                                    .heightIn(min = 48.dp)
                                    .testTag("overview-stop-data-only"),
                                onClick = onStopDataPlaneOnly,
                            ) {
                                Text(contracts.i18n.translate(locale, "android.vpn.stop_data_only"))
                            }
                        }
                    }
                }
                selectedNetworkName?.let { name ->
                    Text(
                        modifier = Modifier.testTag("overview-network"),
                        text = name,
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }

        // On-device embedded Rust node status & live telemetry card
        if (embeddedEndpoint != null || onStartEmbeddedNode != null) {
            Card(
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("overview-embedded-node"),
            ) {
                Column(
                    modifier = Modifier.padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text(
                        modifier = Modifier.semantics { heading() },
                        text = contracts.i18n.translate(locale, "android.embedded.title"),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        modifier = Modifier.testTag("overview-embedded-status"),
                        text = contracts.i18n.translate(
                            locale,
                            if (embeddedEndpoint != null) "android.embedded.running" else "android.embedded.stopped",
                        ),
                        style = MaterialTheme.typography.bodyMedium,
                    )
                    embeddedEndpoint?.let { ep ->
                        Text(
                            modifier = Modifier.testTag("overview-embedded-nodeid"),
                            text = "node=${ep.nodeId.take(18)} · ${ep.apiUrl}",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    nativeTelemetry?.let { t ->
                        Text(
                            modifier = Modifier.testTag("overview-embedded-telemetry"),
                            text = "uptime=${t.uptimeSecs}s · tunnels=${t.activeTunnelSessions} · RX ${t.rxPackets}pkts/${t.rxBytes}B · TX ${t.txPackets}pkts/${t.txBytes}B",
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    if (embeddedEndpoint == null && onStartEmbeddedNode != null) {
                        Button(
                            modifier = Modifier
                                .heightIn(min = 48.dp)
                                .testTag("overview-start-embedded"),
                            onClick = onStartEmbeddedNode,
                        ) {
                            Text(contracts.i18n.translate(locale, "android.embedded.start"))
                        }
                    }
                }
            }
        }

        // Battery, Doze, OEM restriction & metered-network health card (W16-06)
        if (deviceEnv != null && devicePolicy != null) {
            Card(
                modifier = Modifier
                    .fillMaxWidth()
                    .testTag("overview-device-health"),
            ) {
                Column(
                    modifier = Modifier.padding(16.dp),
                    verticalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Text(
                        modifier = Modifier.semantics { heading() },
                        text = contracts.i18n.translate(locale, "android.device.health.title"),
                        style = MaterialTheme.typography.titleMedium,
                    )
                    Text(
                        modifier = Modifier.testTag("device-battery-opt-status"),
                        text = contracts.i18n.translate(
                            locale,
                            if (deviceEnv.ignoringBatteryOptimizations) {
                                "android.device.battery_opt_exempt"
                            } else {
                                "android.device.battery_opt_active"
                            },
                        ),
                        style = MaterialTheme.typography.bodySmall,
                        color = if (deviceEnv.ignoringBatteryOptimizations) {
                            MaterialTheme.colorScheme.primary
                        } else {
                            MaterialTheme.colorScheme.error
                        },
                    )
                    Text(
                        modifier = Modifier.testTag("device-env-summary"),
                        text = "${deviceEnv.manufacturer} API ${deviceEnv.androidSdk} · net=${deviceEnv.transportKind}${if (deviceEnv.isMetered) " (metered)" else ""} · bat=${deviceEnv.batteryPercent}% · mode=${devicePolicy.degradation.name}",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    if (deviceEnv.isMetered || deviceEnv.isRoaming) {
                        Text(
                            modifier = Modifier.testTag("device-metered-guard"),
                            text = contracts.i18n.translate(locale, "android.device.metered_guard"),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.tertiary,
                        )
                    }
                    Text(
                        text = contracts.i18n.translate(locale, "android.device.impact_note"),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    if (!deviceEnv.ignoringBatteryOptimizations && onOpenBatterySettings != null) {
                        OutlinedButton(
                            modifier = Modifier
                                .heightIn(min = 48.dp)
                                .testTag("device-open-battery-settings"),
                            onClick = onOpenBatterySettings,
                        ) {
                            Text(contracts.i18n.translate(locale, "android.device.open_battery_settings"))
                        }
                    }
                }
            }
        }
    }

    if (showVpnConsentExplain) {
        AlertDialog(
            onDismissRequest = { showVpnConsentExplain = false },
            title = {
                Text(contracts.i18n.translate(locale, "android.vpn.consent.title"))
            },
            text = {
                Text(
                    contracts.i18n.translate(locale, "android.vpn.consent.detail"),
                    style = MaterialTheme.typography.bodySmall,
                )
            },
            confirmButton = {
                TextButton(
                    modifier = Modifier.testTag("vpn-consent-confirm"),
                    onClick = {
                        showVpnConsentExplain = false
                        onTunnelControl?.invoke(TunnelControl.REQUEST_CONSENT)
                    },
                ) {
                    Text(contracts.i18n.translate(locale, "android.vpn.consent.continue"))
                }
            },
            dismissButton = {
                TextButton(onClick = { showVpnConsentExplain = false }) {
                    Text(contracts.i18n.translate(locale, "common.cancel"))
                }
            },
        )
    }
}
