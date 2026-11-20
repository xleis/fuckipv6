package cc.xlei.fuckipv6.android.service

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import cc.xlei.fuckipv6.android.MainActivity
import cc.xlei.fuckipv6.android.R

/**
 * Foreground VpnService for the tunnel.
 *
 * The service only owns platform concerns (TUN interface, foreground state,
 * notification); the lifecycle decisions live in [TunnelEngine] so they can be
 * tested without a device. While no packet transport is bound the engine fails
 * closed, which means this service never reports a tunnel that cannot carry
 * packets.
 */
class FuckIpv6VpnService : VpnService() {
    private var preparedNetwork: String? = null

    private val engine: TunnelEngine by lazy {
        TunnelEngine(
            interfaces = TunnelInterfaceFactory { config -> establishInterface(config) },
            transport = {
                TunnelTransports.current?.let { transport -> TransportAdapter(transport) }
            },
        )
    }

    override fun onCreate() {
        super.onCreate()
        ensureChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            TunnelActions.ACTION_START -> startTunnel(intent)
            TunnelActions.ACTION_STOP -> stopTunnel()
            else -> return START_NOT_STICKY
        }
        return START_NOT_STICKY
    }

    private fun startTunnel(intent: Intent) {
        val config = configFrom(intent)
        if (config == null) {
            finish(TunnelState.Failed("state.error.detail"))
            return
        }
        preparedNetwork = config.networkName
        promoteToForeground(config)
        finish(engine.start(config))
    }

    private fun stopTunnel() {
        finish(engine.stop())
    }

    /**
     * Reflects the engine's decision: a running tunnel keeps the foreground
     * notification, anything else tears it down and stops the service.
     */
    private fun finish(state: TunnelState) {
        when (state) {
            is TunnelState.Running -> notifyState(state)
            else -> {
                preparedNetwork = null
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
        }
    }

    override fun onRevoke() {
        finish(engine.revoked())
        super.onRevoke()
    }

    override fun onDestroy() {
        if (TunnelStateStore.state.isActive) {
            TunnelStateStore.apply(TunnelState.Stopped)
        }
        super.onDestroy()
    }

    /** Real TUN establishment, kept out of the engine so the engine stays pure. */
    private fun establishInterface(config: TunnelConfig): TunnelInterfaceHandle? {
        val builder = Builder()
            .setSession(config.networkName)
            .setMtu(config.mtu)
            .addAddress(config.addressV4.substringBefore('/'), prefix(config.addressV4))
            .addRoute("0.0.0.0", 0)
        config.addressV6?.let { builder.addAddress(it.substringBefore('/'), prefix(it)) }
        config.dnsServers.forEach { builder.addDnsServer(it) }
        val descriptor = try {
            builder.establish()
        } catch (error: Exception) {
            Log.w(TAG, "interface establishment failed: ${error.javaClass.simpleName}")
            null
        } ?: return null
        return TunHandle(descriptor)
    }

    private fun configFrom(intent: Intent): TunnelConfig? {
        val networkName = intent.getStringExtra(EXTRA_NETWORK_NAME) ?: return null
        val addressV4 = intent.getStringExtra(EXTRA_ADDRESS_V4) ?: return null
        return TunnelConfig(
            networkName = networkName,
            addressV4 = addressV4,
            addressV6 = intent.getStringExtra(EXTRA_ADDRESS_V6),
            mtu = intent.getIntExtra(EXTRA_MTU, DEFAULT_MTU),
            dnsServers = intent.getStringArrayListExtra(EXTRA_DNS) ?: emptyList(),
        )
    }

    private fun prefix(value: String): Int = value.substringAfter('/', "32").toIntOrNull() ?: 32

    private fun ensureChannel() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val manager = getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(CHANNEL_ID) != null) return
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL_ID, "FuckIPv6 tunnel", NotificationManager.IMPORTANCE_LOW),
        )
    }

    private fun promoteToForeground(config: TunnelConfig) {
        val openApp = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stop = PendingIntent.getService(
            this,
            1,
            Intent(this, FuckIpv6VpnService::class.java).setAction(TunnelActions.ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val notification = android.app.Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(config.networkName)
            .setContentText("正在建立隧道 / preparing tunnel")
            .setSmallIcon(R.drawable.ic_brand_mark)
            .setContentIntent(openApp)
            .addAction(
                android.app.Notification.Action.Builder(null, "停止 / Stop", stop).build(),
            )
            .setOngoing(true)
            .build()
        startForeground(NOTIFICATION_ID, notification)
    }

    private fun notifyState(state: TunnelState) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) !=
            android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val manager = getSystemService(NotificationManager::class.java) ?: return
        val title = preparedNetwork ?: getString(R.string.app_name)
        val text = if (state is TunnelState.Running) {
            "隧道运行中 / tunnel running"
        } else {
            "隧道不可用 / tunnel unavailable"
        }
        val notification = android.app.Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(title)
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_brand_mark)
            .setOngoing(true)
            .build()
        runCatching { manager.notify(NOTIFICATION_ID, notification) }
    }

    /** Wraps the platform descriptor so the engine only sees a handle. */
    private class TunHandle(private val descriptor: ParcelFileDescriptor) : TunnelInterfaceHandle {
        fun fileDescriptor(): ParcelFileDescriptor = descriptor
        override fun close() {
            runCatching { descriptor.close() }
        }
    }

    /** Adapts the app-level transport to the engine's narrower contract. */
    private class TransportAdapter(
        private val transport: TunnelTransport,
    ) : TunnelTransportHandle {
        override fun start(config: TunnelConfig, handle: TunnelInterfaceHandle): Result<Unit> {
            val descriptor = (handle as? TunHandle)?.fileDescriptor()
                ?: return Result.failure(IllegalStateException("unexpected tunnel handle"))
            return transport.start(config, descriptor)
        }

        override fun stop() = transport.stop()
    }

    companion object {
        const val CHANNEL_ID = "fuckipv6-tunnel"
        const val NOTIFICATION_ID = 41
        const val DEFAULT_MTU = 1400
        const val EXTRA_NETWORK_NAME = "network_name"
        const val EXTRA_ADDRESS_V4 = "address_v4"
        const val EXTRA_ADDRESS_V6 = "address_v6"
        const val EXTRA_MTU = "mtu"
        const val EXTRA_DNS = "dns"
        private const val TAG = "FuckIpv6Vpn"

        fun startIntent(context: android.content.Context, config: TunnelConfig): Intent =
            Intent(context, FuckIpv6VpnService::class.java)
                .setAction(TunnelActions.ACTION_START)
                .putExtra(EXTRA_NETWORK_NAME, config.networkName)
                .putExtra(EXTRA_ADDRESS_V4, config.addressV4)
                .putExtra(EXTRA_ADDRESS_V6, config.addressV6)
                .putExtra(EXTRA_MTU, config.mtu)
                .putStringArrayListExtra(EXTRA_DNS, ArrayList(config.dnsServers))

        fun stopIntent(context: android.content.Context): Intent =
            Intent(context, FuckIpv6VpnService::class.java)
                .setAction(TunnelActions.ACTION_STOP)
    }
}
