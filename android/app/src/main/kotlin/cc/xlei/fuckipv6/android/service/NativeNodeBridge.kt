package cc.xlei.fuckipv6.android.service

import android.util.Log
import org.json.JSONObject

/**
 * JNI bridge to `libfuckipv6_android.so` (crates/android-bridge).
 *
 * When running in plain JVM unit tests where no native `.so` is packaged,
 * loading degrades cleanly (`isLibraryLoaded == false`) so pure lifecycle tests
 * keep working without mocks.
 */
object NativeNodeBridge {
    private const val TAG = "FuckIpv6Native"
    private const val LIB_NAME = "fuckipv6_android"

    interface HostVpnCallback {
        /** Applies `settingsJson` through `VpnService.Builder` and returns the detached TUN fd (>= 0) or < 0 on failure. */
        fun openNetwork(settingsJson: String): Int

        /** Releases host-side VPN metadata for `networkName`. Must be idempotent. */
        fun closeNetwork(networkName: String)
    }

    data class StartResult(
        val handle: Long,
        val apiUrl: String,
        val token: String,
        val nodeId: String,
    )

    data class NativeTelemetry(
        val uptimeSecs: Long,
        val activeTunnelSessions: Int,
        val rxPackets: Long,
        val rxBytes: Long,
        val txPackets: Long,
        val txBytes: Long,
        val droppedPackets: Long,
    )

    val isLibraryLoaded: Boolean by lazy {
        try {
            System.loadLibrary(LIB_NAME)
            val version = nativeAbiVersion()
            runCatching { Log.i(TAG, "loaded $LIB_NAME (ABI v$version)") }
            version == 1
        } catch (error: UnsatisfiedLinkError) {
            runCatching { Log.w(TAG, "native library $LIB_NAME unavailable: ${error.message}") }
            false
        } catch (error: SecurityException) {
            runCatching { Log.w(TAG, "native library $LIB_NAME refused: ${error.message}") }
            false
        }
    }

    fun startRuntime(storageRoot: String, callback: HostVpnCallback): Result<StartResult> {
        if (!isLibraryLoaded) {
            return Result.failure(IllegalStateException("native_library_unavailable"))
        }
        val raw = runCatching { nativeStartRuntime(storageRoot, callback) }
            .getOrElse { return Result.failure(it) }
        return parseStartResult(raw)
    }

    fun stopRuntime(handle: Long): Result<Unit> {
        if (!isLibraryLoaded || handle == 0L) return Result.success(Unit)
        val raw = runCatching { nativeStopRuntime(handle) }
            .getOrElse { return Result.failure(it) }
        val json = runCatching { JSONObject(raw) }
            .getOrElse { return Result.failure(it) }
        return if (json.optBoolean("ok", false)) {
            Result.success(Unit)
        } else {
            Result.failure(
                IllegalStateException(json.optString("message", "native_stop_failed")),
            )
        }
    }

    fun notifyNetworkPathChanged(handle: Long): Boolean {
        if (!isLibraryLoaded || handle == 0L) return false
        return runCatching { nativeNotifyNetworkPathChanged(handle) }.getOrDefault(false)
    }

    fun getTelemetry(handle: Long): NativeTelemetry? {
        if (!isLibraryLoaded || handle == 0L) return null
        val raw = runCatching { nativeGetTelemetryJson(handle) }.getOrNull() ?: return null
        return parseTelemetry(raw)
    }

    internal fun parseStartResult(raw: String): Result<StartResult> = runCatching {
        val json = JSONObject(raw)
        if (!json.optBoolean("ok", false)) {
            val code = json.optString("code", "unavailable")
            val message = json.optString("message", "native_start_failed")
            throw IllegalStateException("$code: $message")
        }
        val handle = json.optLong("handle", 0L)
        val apiUrl = json.optString("api_url", "")
        val token = json.optString("token", "")
        val nodeId = json.optString("node_id", "")
        require(handle != 0L && apiUrl.startsWith("http://127.0.0.1:") && token.length >= 32) {
            "malformed nativeStartRuntime response"
        }
        StartResult(handle = handle, apiUrl = apiUrl, token = token, nodeId = nodeId)
    }

    internal fun parseTelemetry(raw: String): NativeTelemetry? = runCatching {
        val json = JSONObject(raw)
        val counters = json.optJSONObject("counters") ?: return null
        NativeTelemetry(
            uptimeSecs = json.optLong("uptime_secs", 0L),
            activeTunnelSessions = json.optInt("active_tunnel_sessions", 0),
            rxPackets = counters.optLong("rx_packets", 0L),
            rxBytes = counters.optLong("rx_bytes", 0L),
            txPackets = counters.optLong("tx_packets", 0L),
            txBytes = counters.optLong("tx_bytes", 0L),
            droppedPackets = counters.optLong("dropped_packets", 0L),
        )
    }.getOrNull()

    @JvmStatic
    private external fun nativeAbiVersion(): Int

    @JvmStatic
    private external fun nativeStartRuntime(
        storageRoot: String,
        hostCallback: HostVpnCallback,
    ): String

    @JvmStatic
    private external fun nativeStopRuntime(handle: Long): String

    @JvmStatic
    private external fun nativeNotifyNetworkPathChanged(handle: Long): Boolean

    @JvmStatic
    private external fun nativeGetTelemetryJson(handle: Long): String
}
