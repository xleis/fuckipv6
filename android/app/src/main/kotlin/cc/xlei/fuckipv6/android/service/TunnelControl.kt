package cc.xlei.fuckipv6.android.service

/**
 * What the shell should offer for the tunnel right now.
 *
 * The decision is pure so the UI cannot invent a "connected" button: the app
 * only offers `REQUEST_CONSENT` or `START` when a transport is actually bound,
 * and otherwise says `UNAVAILABLE` instead of starting a service that would
 * fail closed a moment later.
 */
enum class TunnelControl {
    /** No packet transport is compiled in; starting would fail closed. */
    UNAVAILABLE,

    /** The platform VPN consent dialog has not been granted yet. */
    REQUEST_CONSENT,

    /** Consent is granted and the tunnel is not active: start it. */
    START,

    /** The tunnel is preparing, running or stopping: offer nothing but status. */
    WAITING,

    /** The tunnel is running: offer stop. */
    STOP,
}

object TunnelControlPlan {
    fun decide(
        consentGranted: Boolean,
        transportBound: Boolean,
        state: TunnelState,
    ): TunnelControl {
        if (state is TunnelState.Failed) {
            // A failure is explained by the state itself; a transport that is
            // missing is not something the operator can fix from the UI.
            return if (transportBound) {
                if (consentGranted) TunnelControl.START else TunnelControl.REQUEST_CONSENT
            } else {
                TunnelControl.UNAVAILABLE
            }
        }
        if (state == TunnelState.Running) return TunnelControl.STOP
        if (state.isActive) return TunnelControl.WAITING
        if (!transportBound) return TunnelControl.UNAVAILABLE
        return if (consentGranted) TunnelControl.START else TunnelControl.REQUEST_CONSENT
    }
}
