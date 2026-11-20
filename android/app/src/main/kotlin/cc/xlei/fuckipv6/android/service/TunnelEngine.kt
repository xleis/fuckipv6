package cc.xlei.fuckipv6.android.service

/** The established TUN interface, abstracted so the engine is testable. */
interface TunnelInterfaceHandle {
    fun close()
}

/** Creates the TUN interface. Returns null when the OS refuses. */
fun interface TunnelInterfaceFactory {
    fun establish(config: TunnelConfig): TunnelInterfaceHandle?
}

/**
 * The packet path. Abstracted for the same reason as the interface: the
 * lifecycle must be verifiable without a device.
 */
interface TunnelTransportHandle {
    fun start(config: TunnelConfig, handle: TunnelInterfaceHandle): Result<Unit>
    fun stop()
}

/**
 * Tunnel lifecycle engine.
 *
 * Order matters and is enforced here rather than in the service: a tunnel only
 * becomes `Running` after the interface exists *and* the transport accepted it.
 * Every failure path closes the interface and stops the transport, and a
 * duplicate start or stop is a no-op rather than a second side effect.
 */
class TunnelEngine(
    private val interfaces: TunnelInterfaceFactory,
    private val transport: () -> TunnelTransportHandle?,
) {
    private var handle: TunnelInterfaceHandle? = null
    fun start(config: TunnelConfig): TunnelState {
        if (TunnelStateStore.state is TunnelState.Running ||
            TunnelStateStore.state is TunnelState.Preparing
        ) {
            return TunnelStateStore.state
        }
        TunnelStateStore.apply(TunnelState.Preparing)
        val configError = config.validationError()
        if (configError != null) {
            return fail(configError)
        }
        val transport = transport()
        if (transport == null) {
            // No packet path: establishing TUN would report a tunnel that drops
            // everything.
            return fail("state.unsupported.detail")
        }
        val established = interfaces.establish(config) ?: return fail("state.error.detail")
        handle = established
        val started = runCatching { transport.start(config, established) }
        if (started.isFailure || started.getOrNull()?.isFailure == true) {
            cleanup()
            return fail("state.error.detail")
        }
        return TunnelStateStore.apply(TunnelState.Running)
    }

    fun stop(): TunnelState {
        val current = TunnelStateStore.state
        if (current is TunnelState.Stopped) return current
        TunnelStateStore.apply(TunnelState.Stopping)
        cleanup()
        return TunnelStateStore.apply(TunnelState.Stopped)
    }

    /** The OS withdrew the interface: report a failure, never "stopped cleanly". */
    fun revoked(): TunnelState {
        cleanup()
        return TunnelStateStore.apply(TunnelState.Failed("state.offline.detail"))
    }

    /** Stops the transport and closes the interface exactly once. */
    private fun cleanup() {
        runCatching { transport()?.stop() }
        runCatching { handle?.close() }
        handle = null
    }

    private fun fail(reasonKey: String): TunnelState =
        TunnelStateStore.apply(TunnelState.Failed(reasonKey))
}
