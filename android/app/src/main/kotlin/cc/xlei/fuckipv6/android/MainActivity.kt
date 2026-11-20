package cc.xlei.fuckipv6.android

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.MutableStateFlow
import java.util.UUID
import java.util.concurrent.atomic.AtomicLong
import cc.xlei.fuckipv6.android.bridge.AdminRecoverySurface
import cc.xlei.fuckipv6.android.bridge.BridgeState
import cc.xlei.fuckipv6.android.bridge.CommandJson
import cc.xlei.fuckipv6.android.bridge.CommandOutcome
import cc.xlei.fuckipv6.android.bridge.RecoveryStatusView
import cc.xlei.fuckipv6.android.bridge.HttpSnapshotSource
import cc.xlei.fuckipv6.android.bridge.NodeApiConfig
import cc.xlei.fuckipv6.android.bridge.NodeBridge
import cc.xlei.fuckipv6.android.bridge.NodeCommandBridge
import cc.xlei.fuckipv6.android.bridge.NodeCommandClient
import cc.xlei.fuckipv6.android.bridge.RelaySurface
import cc.xlei.fuckipv6.android.ui.AdminRecoveryPageData
import cc.xlei.fuckipv6.android.ui.CoordinatorPageData
import cc.xlei.fuckipv6.android.ui.RelayPageData
import kotlinx.coroutines.launch
import cc.xlei.fuckipv6.android.contract.SharedContractLoader
import cc.xlei.fuckipv6.android.ui.ContractFailureScreen
import cc.xlei.fuckipv6.android.ui.FuckIpv6App
import cc.xlei.fuckipv6.android.ui.FuckIpv6Theme

class MainActivity : ComponentActivity() {
    private val bridgeScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var pollJob: Job? = null
    private var bridge: NodeBridge? = null
    private var commandBridge: NodeCommandBridge? = null
    private var recoveryBridge: NodeCommandBridge? = null
    private val requestSequence = AtomicLong(System.currentTimeMillis())
    private val recoveries = MutableStateFlow<List<RecoveryStatusView>>(emptyList())
    private val adminPage = MutableStateFlow<AdminRecoveryPageData?>(null)
    private var refreshNetwork: String? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        val contracts = runCatching { SharedContractLoader.load(assets) }

        val apiConfig = loadApiConfig(intent)
        val node = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeBridge(HttpSnapshotSource(it)) }
            ?.also { candidate ->
                bridge = candidate
                pollJob = candidate.start(bridgeScope)
            }
        commandBridge = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeCommandBridge(NodeCommandClient(it)) }
        recoveryBridge = apiConfig
            ?.takeIf { it.isValid }
            ?.let { NodeCommandBridge(NodeCommandClient(it)) }
            ?.also { adminBridge ->
                bridgeScope.launch {
                    adminBridge.lastOutcome().collect { outcome ->
                        if (outcome !is CommandOutcome.Succeeded) return@collect
                        when (outcome.outcomeStatus) {
                            "admin_recoveries" -> recoveries.value =
                                AdminRecoverySurface.parseList(outcome.data)
                            "admin_recovery_status" -> {
                                val status = AdminRecoverySurface.parseSingle(outcome.data)
                                if (status != null) {
                                    recoveries.value = listOf(status) + recoveries.value
                                        .filterNot { it.requestId == status.requestId }
                                    refreshNetwork?.let(::refreshRecoveries)
                                }
                            }
                        }
                    }
                }
            }
        bridgeScope.launch {
            node?.state()?.collect { state ->
                val ready = state as? BridgeState.Ready ?: return@collect
                val networkName = ready.relay?.networkName
                if (networkName != null) {
                    val networkId = ready.networks
                        .firstOrNull { it.networkName == networkName }
                        ?.networkId
                    adminPage.value = AdminRecoveryPageData.fromBridge(
                        networkName = networkName,
                        networkId = networkId,
                        recoveries = recoveries.value,
                        onSubmit = { command ->
                            submitRecovery(
                                AdminRecoverySurface.submitCommand(
                                    command.network_name,
                                    command.request,
                                ),
                            )
                        },
                        onGetStatus = { command ->
                            submitRecovery(
                                AdminRecoverySurface.statusCommand(
                                    command.network_name,
                                    command.request_id,
                                ),
                            )
                        },
                        onList = { command -> submitRecovery(AdminRecoverySurface.listCommand(command.network_name)) },
                        onRevoke = { command ->
                            submitRecovery(
                                AdminRecoverySurface.revokeCommand(
                                    command.network_name,
                                    command.request_id,
                                    command.reason,
                                ),
                            )
                        },
                        onExecute = { command ->
                            submitRecovery(
                                AdminRecoverySurface.executeCommand(
                                    command.network_name,
                                    command.request_id,
                                ),
                            )
                        },
                    )
                    if (refreshNetwork != networkName) {
                        refreshNetwork = networkName
                        refreshRecoveries(networkName)
                    }
                }
            }
        }

        setContent {
            val bridgeState by remember(node) {
                node?.state() ?: MutableStateFlow<BridgeState>(BridgeState.NotConfigured)
            }.collectAsState()
            val ready = bridgeState as? BridgeState.Ready
            val files = ready?.data
            val filesFailureKey = (bridgeState as? BridgeState.Failed)?.failure?.uiStateKey
            val relay = ready?.relay?.let(::relayPageData)
            val coordinator = ready?.coordinator?.let(CoordinatorPageData::fromBridge)
            val admin = adminPage.collectAsState().value
            FuckIpv6Theme {
                contracts.fold(
                    onSuccess = {
                        FuckIpv6App(
                            contracts = it,
                            relay = relay,
                            coordinator = coordinator,
                            adminRecovery = admin,
                            files = files,
                            filesFailureKey = filesFailureKey,
                        )
                    },
                    onFailure = { ContractFailureScreen(it.message) },
                )
            }
        }
    }

    /**
     * Builds the Relay page state from the authoritative snapshot and wires the
     * switch to the command channel. The switch does not assume success: the
     * snapshot poll is what flips `relayEnabled` once the operation completes.
     */
    private fun relayPageData(surface: RelaySurface): RelayPageData = RelayPageData(
        networkName = surface.networkName,
        relayEnabled = surface.relayEnabled,
        health = surface.health,
        governance = surface.governance,
        grantedPermissions = surface.permissions,
        // Without a command channel the page must not offer the action at all.
        backendAvailable = commandBridge != null,
        onSetRelayRole = { enabled ->
            val commands = commandBridge
            if (commands == null) {
                Unit
            } else {
                val body = CommandJson.envelope(
                    command = CommandJson.setNetworkRole(
                        networkName = surface.networkName,
                        role = "relay",
                        enabled = enabled,
                    ),
                    requestId = nextRequestId(),
                    idempotencyKey = "relay-${surface.networkName}-$enabled-${UUID.randomUUID()}",
                )
                commands.submit(bridgeScope, body)
                Unit
            }
        },
    )

    /** Refresh the recovery list through the recovery command bridge. */
    private fun refreshRecoveries(networkName: String) {
        submitRecovery(AdminRecoverySurface.listCommand(networkName), networkName)
    }

    /**
     * One command at a time on the recovery bridge. The idempotency key keeps a
     * retried submit from executing twice; the UI never infers success — the
     * operation has to report a terminal stage.
     */
    private fun submitRecovery(command: org.json.JSONObject, label: String? = null) {
        val bridge = recoveryBridge ?: return
        val kind = command.optString("command", "admin_recovery")
        val body = CommandJson.envelope(
            command = command,
            requestId = nextRequestId(),
            idempotencyKey = "$kind-${label ?: refreshNetwork ?: "-"}-${UUID.randomUUID()}",
        )
        bridge.submit(bridgeScope, body)
    }

    private fun nextRequestId(): Long {
        val candidate = requestSequence.incrementAndGet()
        return if (candidate > 0) candidate else 1
    }

    override fun onDestroy() {
        pollJob?.cancel()
        bridgeScope.cancel()
        bridge = null
        super.onDestroy()
    }

    /**
     * A node URL and token are configuration, not defaults: they come from the
     * launching intent (tooling, tests) or the app's own preferences. Without
     * them the app shows the unavailable surface instead of probing a guessed
     * address.
     */
    private fun loadApiConfig(intent: Intent?): NodeApiConfig? {
        val preferences = getSharedPreferences(PREFERENCES, MODE_PRIVATE)
        val url = intent?.getStringExtra(EXTRA_API_URL)
            ?.takeIf { it.isNotBlank() }
            ?: preferences.getString(KEY_API_URL, null)
        if (url.isNullOrBlank()) return null
        val token = intent?.getStringExtra(EXTRA_API_TOKEN)
            ?.takeIf { it.isNotBlank() }
            ?: preferences.getString(KEY_API_TOKEN, null)
        return NodeApiConfig(baseUrl = url, token = token)
    }

    companion object {
        const val EXTRA_API_URL = "node_api_url"
        const val EXTRA_API_TOKEN = "node_api_token"
        private const val PREFERENCES = "fuckipv6.node"
        private const val KEY_API_URL = "node_api_url"
        private const val KEY_API_TOKEN = "node_api_token"
    }
}
