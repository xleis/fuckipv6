package cc.xlei.fuckipv6.android.service

import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject

/** Loopback endpoint published by the in-process embedded Rust node. */
data class EmbeddedNodeEndpoint(
    val apiUrl: String,
    val token: String,
    val nodeId: String,
    val startedAtUnixMs: Long,
)

/** Parsed `AndroidTunnelSettings` JSON delivered from Rust's `EmbeddedPlatform::open_network`. */
data class NativeTunnelSettings(
    val network: String,
    val mtu: Int,
    val routingMode: String,
    val ipv4Address: String?,
    val ipv4Prefix: Int,
    val ipv6Address: String?,
    val ipv6Prefix: Int,
    val routes: List<String>,
    val dns: List<String>,
    val disallowSelfPackage: Boolean,
) {
    companion object {
        fun parse(raw: String): NativeTunnelSettings? = runCatching {
            val json = JSONObject(raw)
            val network = json.optString("network", "").trim()
            if (network.isEmpty()) return null
            val mtu = json.optInt("mtu", FuckIpv6VpnService.DEFAULT_MTU)
            if (mtu !in 1280..9000) return null
            val ipv4Obj = json.optJSONObject("ipv4")
            val ipv6Obj = json.optJSONObject("ipv6")
            val routes = parseStringArray(json.optJSONArray("routes"))
            val dns = parseStringArray(json.optJSONArray("dns"))
            NativeTunnelSettings(
                network = network,
                mtu = mtu,
                routingMode = json.optString("routing_mode", "mesh_only"),
                ipv4Address = ipv4Obj?.optString("address", "")?.takeIf { it.isNotBlank() },
                ipv4Prefix = ipv4Obj?.optInt("prefix", 24) ?: 24,
                ipv6Address = ipv6Obj?.optString("address", "")?.takeIf { it.isNotBlank() },
                ipv6Prefix = ipv6Obj?.optInt("prefix", 64) ?: 64,
                routes = routes,
                dns = dns,
                disallowSelfPackage = json.optBoolean("disallow_self_package", true),
            )
        }.getOrNull()

        private fun parseStringArray(array: JSONArray?): List<String> {
            if (array == null) return emptyList()
            return (0 until array.length()).mapNotNull { idx ->
                array.optString(idx, "").trim().takeIf { it.isNotEmpty() }
            }
        }
    }
}

/**
 * Process-scoped holder owned by [FuckIpv6VpnService] (never by `MainActivity`).
 *
 * Closing `MainActivity` keeps both the embedded Rust node and the foreground
 * VPN data plane running. Stopping the data plane (`stop_node`) releases TUN,
 * routes and DNS without tearing down the loopback control HTTP API.
 */
object EmbeddedNodeHost {
    private val lock = Any()

    @Volatile
    private var nativeHandle: Long = 0L

    private val endpointFlow = MutableStateFlow<EmbeddedNodeEndpoint?>(null)
    val endpoint: StateFlow<EmbeddedNodeEndpoint?> = endpointFlow.asStateFlow()

    private val telemetryFlow = MutableStateFlow<NativeNodeBridge.NativeTelemetry?>(null)
    val telemetry: StateFlow<NativeNodeBridge.NativeTelemetry?> = telemetryFlow.asStateFlow()

    private val lastErrorFlow = MutableStateFlow<String?>(null)
    val lastError: StateFlow<String?> = lastErrorFlow.asStateFlow()

    val isRunning: Boolean
        get() = nativeHandle != 0L && endpointFlow.value != null

    fun ensureStarted(
        storageRoot: String,
        callback: NativeNodeBridge.HostVpnCallback,
    ): Result<EmbeddedNodeEndpoint> {
        synchronized(lock) {
            endpointFlow.value?.let { existing ->
                if (nativeHandle != 0L) return Result.success(existing)
            }
            val started = NativeNodeBridge.startRuntime(storageRoot, callback)
            return started.map { result ->
                nativeHandle = result.handle
                lastErrorFlow.value = null
                val ep = EmbeddedNodeEndpoint(
                    apiUrl = result.apiUrl,
                    token = result.token,
                    nodeId = result.nodeId,
                    startedAtUnixMs = System.currentTimeMillis(),
                )
                endpointFlow.value = ep
                ep
            }.onFailure { error ->
                lastErrorFlow.value = error.message ?: "embedded_start_failed"
            }
        }
    }

    fun notifyNetworkPathChanged(): Boolean {
        val handle = nativeHandle
        if (handle == 0L) return false
        return NativeNodeBridge.notifyNetworkPathChanged(handle)
    }

    fun refreshTelemetry(): NativeNodeBridge.NativeTelemetry? {
        val handle = nativeHandle
        if (handle == 0L) return null
        val snapshot = NativeNodeBridge.getTelemetry(handle)
        telemetryFlow.value = snapshot
        return snapshot
    }

    fun stopAll(): Result<Unit> {
        synchronized(lock) {
            val handle = nativeHandle
            nativeHandle = 0L
            endpointFlow.value = null
            telemetryFlow.value = null
            return if (handle != 0L) {
                NativeNodeBridge.stopRuntime(handle)
            } else {
                Result.success(Unit)
            }
        }
    }
}
