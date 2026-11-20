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
import cc.xlei.fuckipv6.android.bridge.BridgeState
import cc.xlei.fuckipv6.android.bridge.HttpSnapshotSource
import cc.xlei.fuckipv6.android.bridge.NodeApiConfig
import cc.xlei.fuckipv6.android.bridge.NodeBridge
import cc.xlei.fuckipv6.android.contract.SharedContractLoader
import cc.xlei.fuckipv6.android.ui.ContractFailureScreen
import cc.xlei.fuckipv6.android.ui.FuckIpv6App
import cc.xlei.fuckipv6.android.ui.FuckIpv6Theme

class MainActivity : ComponentActivity() {
    private val bridgeScope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var pollJob: Job? = null
    private var bridge: NodeBridge? = null

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

        setContent {
            val bridgeState by remember(node) {
                node?.state() ?: MutableStateFlow<BridgeState>(BridgeState.NotConfigured)
            }.collectAsState()
            val files = (bridgeState as? BridgeState.Ready)?.data
            val filesFailureKey = (bridgeState as? BridgeState.Failed)?.failure?.uiStateKey
            FuckIpv6Theme {
                contracts.fold(
                    onSuccess = {
                        FuckIpv6App(
                            contracts = it,
                            files = files,
                            filesFailureKey = filesFailureKey,
                        )
                    },
                    onFailure = { ContractFailureScreen(it.message) },
                )
            }
        }
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
