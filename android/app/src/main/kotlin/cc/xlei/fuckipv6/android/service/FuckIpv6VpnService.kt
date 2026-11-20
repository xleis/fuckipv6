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
 * Foreground VpnService skeleton for the tunnel.
 *
 * The service owns the TUN interface and the notification; the packet transport
 * is a separate, currently unbound dependency. Until a transport is bound the
 * service fails closed and stops itself rather than reporting a tunnel that
 * cannot carry packets.
 */
class FuckIpv6VpnService : VpnService() {
    private var tun: ParcelFileDescriptor? = null
    private var configuredNetwork: String? = null

    override fun onCreate() {
        super.onCreate()
        ensureChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val action = intent?.action
        if (!TunnelActions.isKnown(action)) {
            return START_NOT_STICKY
        }
        when (action) {
            TunnelActions.ACTION_START -> startTunnel(intent)
            TunnelActions.ACTION_STOP -> stopTunnel()
        }
        return START_NOT_STICKY
    }

    private fun startTunnel(intent: Intent?) {
        if (TunnelStateStore.state is TunnelState.Running) {
            return
        }
        TunnelStateStore.apply(TunnelState.Preparing)
        val config = configFrom(intent)
        val configError = config?.validationError()
        if (config == null || configError != null) {
            fail(configError ?: "state.error.detail")
            return
        }
        val transport = TunnelTransports.current
        if (transport == null) {
            // No packet path exists yet: establishing TUN would be a lie.
            fail("state.unsupported.detail")
            return
        }
        promoteToForeground(config)
        val builder = Builder()
            .setSession(config.networkName)
            .setMtu(config.mtu)
            .addAddress(config.addressV4.substringBefore('/'), prefix(config.addressV4))
            .addRoute("0.0.0.0", 0)
        config.addressV6?.let { v6 ->
            builder.addAddress(v6.substringBefore('/'), prefix(v6))
        }
        config.dnsServers.forEach { builder.addDnsServer(it) }
        val established = try {
            builder.establish()
        } catch (error: Exception) {
            logQuietly("establish failed", error)
            null
        }
        if (established == null) {
            fail("state.error.detail")
            return
        }
        val started = runCatching { transport.start(config, established) }
        if (started.isFailure) {
            runCatching { established.close() }
            fail("state.error.detail")
            return
        }
        tun = established
        configuredNetwork = config.networkName
        TunnelStateStore.apply(TunnelState.Running)
        notifyState(R.string.app_name, "隧道运行中 / tunnel running")
    }

    private fun stopTunnel() {
        TunnelStateStore.apply(TunnelState.Stopping)
        runCatching { TunnelTransports.current?.stop() }
        runCatching { tun?.close() }
        tun = null
        configuredNetwork = null
        TunnelStateStore.apply(TunnelState.Stopped)
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    private fun fail(reasonKey: String) {
        TunnelStateStore.apply(TunnelState.Failed(reasonKey))
        runCatching { TunnelTransports.current?.stop() }
        runCatching { tun?.close() }
        tun = null
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onRevoke() {
        // The OS withdrew the interface: the tunnel is gone whether or not the
        // transport noticed, so report it as failed instead of "stopped".
        fail("state.offline.detail")
        super.onRevoke()
    }

    override fun onDestroy() {
        runCatching { TunnelTransports.current?.stop() }
        runCatching { tun?.close() }
        tun = null
        if (TunnelStateStore.state.isActive) {
            TunnelStateStore.apply(TunnelState.Stopped)
        }
        super.onDestroy()
    }

    private fun configFrom(intent: Intent?): TunnelConfig? {
        intent ?: return null
        val networkName = intent.getStringExtra(EXTRA_NETWORK_NAME) ?: return null
        val addressV4 = intent.getStringExtra(EXTRA_ADDRESS_V4) ?: return null
        val mtu = intent.getIntExtra(EXTRA_MTU, DEFAULT_MTU)
        val dns = intent.getStringArrayListExtra(EXTRA_DNS) ?: emptyList()
        val addressV6 = intent.getStringExtra(EXTRA_ADDRESS_V6)
        return TunnelConfig(networkName, addressV4, addressV6, mtu, dns)
    }

    private fun prefix(value: String): Int =
        value.substringAfter('/', "32").toIntOrNull() ?: 32

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
            .setContentText("隧道已连接 / tunnel connected")
            .setSmallIcon(R.drawable.ic_brand_mark)
            .setContentIntent(openApp)
            .addAction(
                android.app.Notification.Action.Builder(
                    null,
                    "停止 / Stop",
                    stop,
                ).build(),
            )
            .setOngoing(true)
            .build()
        startForeground(NOTIFICATION_ID, notification)
    }

    private fun notifyState(titleRes: Int, text: String) {
        // Posting a non-foreground update needs POST_NOTIFICATIONS on Android 13+.
        // The foreground notification itself is exempt, so a missing grant only
        // costs the progress text, never the tunnel.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) !=
            android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val manager = getSystemService(NotificationManager::class.java) ?: return
        val notification = android.app.Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(getString(titleRes))
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_brand_mark)
            .setOngoing(true)
            .build()
        runCatching { manager.notify(NOTIFICATION_ID, notification) }
    }

    private fun logQuietly(message: String, error: Throwable) {
        // Never log interface addresses or keys; only the failure class.
        Log.w(TAG, "$message: ${error.javaClass.simpleName}")
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

        fun startIntent(
            context: android.content.Context,
            config: TunnelConfig,
        ): Intent = Intent(context, FuckIpv6VpnService::class.java)
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
