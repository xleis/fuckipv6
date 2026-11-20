package cc.xlei.fuckipv6.android.service

/**
 * Tunnel lifecycle the Android side may claim.
 *
 * `Running` is only reached after the VpnService actually established an
 * interface and a transport is bound; without a transport the tunnel fails
 * closed instead of showing a connected state that carries no packets.
 */
sealed interface TunnelState {
    data object Stopped : TunnelState
    data object Preparing : TunnelState
    data object Running : TunnelState
    data object Stopping : TunnelState
    data class Failed(val reasonKey: String) : TunnelState

    val isActive: Boolean
        get() = this is Preparing || this is Running || this is Stopping
}

/** One validated tunnel request, parsed from the service intent. */
data class TunnelConfig(
    val networkName: String,
    val addressV4: String,
    val addressV6: String?,
    val mtu: Int,
    val dnsServers: List<String>,
) {
    /** Returns null when the config is not something this app can hand to TUN. */
    fun validationError(): String? {
        if (networkName.isBlank()) return "state.error.detail"
        if (!addressV4.contains('/')) return "state.error.detail"
        val prefix = addressV4.substringAfter('/').toIntOrNull() ?: return "state.error.detail"
        if (prefix !in 0..32) return "state.error.detail"
        if (mtu !in 576..9000) return "state.error.detail"
        if (dnsServers.any { it.isBlank() }) return "state.error.detail"
        addressV6?.let { if (!it.contains('/')) return "state.error.detail" }
        return null
    }
}

object TunnelActions {
    const val ACTION_START = "cc.xlei.fuckipv6.android.TUNNEL_START"
    const val ACTION_STOP = "cc.xlei.fuckipv6.android.TUNNEL_STOP"

    fun isKnown(action: String?): Boolean = action == ACTION_START || action == ACTION_STOP
}

/**
 * Transition rules. Anything not listed is refused, so a late stop cannot
 * resurrect a stopped tunnel and a duplicate start cannot double-prepare.
 */
object TunnelTransitions {
    fun next(current: TunnelState, requested: TunnelState): TunnelState? = when {
        requested == current -> null
        requested is TunnelState.Preparing && current == TunnelState.Stopped -> requested
        requested is TunnelState.Running && current == TunnelState.Preparing -> requested
        requested is TunnelState.Stopping &&
            (current == TunnelState.Preparing || current == TunnelState.Running) -> requested
        requested is TunnelState.Stopped &&
            (current is TunnelState.Stopping || current is TunnelState.Preparing ||
                current is TunnelState.Running || current is TunnelState.Failed) -> requested
        requested is TunnelState.Failed &&
            (current is TunnelState.Preparing || current is TunnelState.Running) -> requested
        else -> null
    }
}

/** Process-local state holder the UI observes; it never persists across restarts. */
object TunnelStateStore {
    @Volatile
    var state: TunnelState = TunnelState.Stopped
        private set

    fun apply(requested: TunnelState): TunnelState {
        val next = TunnelTransitions.next(state, requested) ?: state
        state = next
        return next
    }

    fun reset() {
        state = TunnelState.Stopped
    }
}
