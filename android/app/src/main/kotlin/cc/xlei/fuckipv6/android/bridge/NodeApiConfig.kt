package cc.xlei.fuckipv6.android.bridge

/**
 * Where the Android app finds the node API it is allowed to talk to.
 *
 * The node control API binds a loopback address by default (`127.0.0.1`), so an
 * app running on the same device can reach it directly and the Android
 * emulator reaches the host through `10.0.2.2`. There is no default URL: an
 * unconfigured app has no bridge at all rather than a guess that silently
 * points somewhere else.
 */
data class NodeApiConfig(
    val baseUrl: String,
    val token: String? = null,
) {
    val snapshotUrl: String
        get() = baseUrl.trimEnd('/') + SNAPSHOT_PATH

    val isValid: Boolean
        get() = validationError() == null

    fun validationError(): String? {
        val trimmed = baseUrl.trim()
        if (trimmed.isEmpty()) return "bridge.error.config_missing"
        val schemeSeparator = trimmed.indexOf("://")
        if (schemeSeparator <= 0) return "bridge.error.config_invalid"
        val scheme = trimmed.substring(0, schemeSeparator).lowercase()
        if (scheme != "http" && scheme != "https") return "bridge.error.config_invalid"
        val authority = trimmed.substring(schemeSeparator + 3).substringBefore('/')
        if (authority.isEmpty()) return "bridge.error.config_invalid"
        if (authority.any { it.isWhitespace() }) return "bridge.error.config_invalid"
        return null
    }

    companion object {
        const val SNAPSHOT_PATH = "/api/v1/snapshot"
    }
}

/** Closed set of bridge failures; every one of them maps to a stable UI key. */
sealed interface BridgeFailure {
    data class Unauthorized(val status: Int) : BridgeFailure
    data class HttpStatus(val status: Int) : BridgeFailure
    data object Unreachable : BridgeFailure
    data object Malformed : BridgeFailure
    data object Misconfigured : BridgeFailure

    /** Stable, machine-readable failure key kept for logs and tests. */
    val key: String
        get() = when (this) {
            is Unauthorized -> "bridge.error.unauthorized"
            is HttpStatus -> "bridge.error.http"
            Unreachable -> "bridge.error.unreachable"
            Malformed -> "bridge.error.malformed"
            Misconfigured -> "bridge.error.config_invalid"
        }

    /**
     * Existing shared surface copy used to render the failure. The bridge does
     * not invent new strings: an unauthorised node token reads as a permission
     * problem, an unreachable or non-200 node reads as offline, and an
     * unparseable body reads as a generic error.
     */
    val uiStateKey: String
        get() = when (this) {
            is Unauthorized -> "state.permission_denied.detail"
            is HttpStatus, Unreachable -> "state.offline.detail"
            Malformed -> "state.error.detail"
            Misconfigured -> "state.unsupported.detail"
        }
}
