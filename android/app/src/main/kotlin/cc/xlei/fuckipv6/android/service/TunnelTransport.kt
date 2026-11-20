package cc.xlei.fuckipv6.android.service

import android.os.ParcelFileDescriptor

/**
 * The piece that would move packets between TUN and the node data plane.
 *
 * Nothing in the app binds one yet: the node packet transport is not exposed to
 * Android (that is W16 work). While this is null the VpnService must fail closed
 * — establishing a TUN without a transport would show a "connected" tunnel that
 * silently drops every packet.
 */
interface TunnelTransport {
    /** Wires the established TUN descriptor. Returning an error fails the tunnel. */
    fun start(config: TunnelConfig, tun: ParcelFileDescriptor): Result<Unit>

    /** Stops the transport. Must be safe to call twice. */
    fun stop()
}

object TunnelTransports {
    @Volatile
    private var active: TunnelTransport? = null

    val current: TunnelTransport?
        get() = active

    fun bind(transport: TunnelTransport) {
        active = transport
    }

    fun unbind() {
        active = null
    }
}
