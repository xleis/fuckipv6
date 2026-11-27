package cc.xlei.fuckipv6.android.service

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.os.Build
import android.os.ParcelFileDescriptor
import android.util.Log
import cc.xlei.fuckipv6.android.MainActivity
import cc.xlei.fuckipv6.android.R
import cc.xlei.fuckipv6.android.bridge.CommandJson
import cc.xlei.fuckipv6.android.bridge.NodeApiConfig
import cc.xlei.fuckipv6.android.bridge.NodeCommandClient
import java.io.File
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors
import org.json.JSONObject

/**
 * Foreground VpnService hosting both the embedded Rust node runtime and the
 * Android TUN data plane.
 *
 * Lifecycle guarantees (W16-02):
 * - Owns the native embedded node handle independently of `MainActivity` so
 *   closing the Activity never drops the tunnel or control service.
 * - Uses `addDisallowedApplication(packageName)` on every TUN establishment so
 *   the node's own QUIC/UDP/TCP/WS sockets bypass Android VPN without routing
 *   loops.
 * - `ACTION_STOP_DATA_PLANE` (`stop node`) stops active networks and releases
 *   TUN/routes/DNS while keeping the loopback control HTTP API reachable.
 * - `ACTION_STOP` releases TUN, routes, DNS, stops the foreground notification
 *   and exits cleanly.
 */
class FuckIpv6VpnService : VpnService(), NativeNodeBridge.HostVpnCallback {
    private var preparedNetwork: String? = null
    private val activeNetworks = ConcurrentHashMap.newKeySet<String>()
    private val worker = Executors.newSingleThreadExecutor { runnable ->
        Thread(runnable, "fv6-vpn-ctl").apply { isDaemon = true }
    }
    private var recoveryObserver: cc.xlei.fuckipv6.android.platform.AndroidNetworkRecoveryObserver? = null

    @Volatile
    private var lastAlert: String? = null

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
        if (NativeNodeBridge.isLibraryLoaded && TunnelTransports.current == null) {
            TunnelTransports.bind(EmbeddedTunnelTransport(this))
        }
        recoveryObserver = cc.xlei.fuckipv6.android.platform.AndroidNetworkRecoveryObserver(
            context = this,
            onPathReconnect = {
                val ok = EmbeddedNodeHost.notifyNetworkPathChanged()
                refreshNotification()
                ok
            },
            onCleanupRequired = { stopCompletely, reason ->
                lastAlert = reason
                if (stopCompletely) {
                    stopDataPlaneKeepControl()
                } else {
                    refreshNotification()
                }
            },
        ).also { it.start() }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val action = intent?.action
        // System restart after process kill with null intent (START_STICKY):
        // restore only if the owner enabled recovery and a tunnel was previously active.
        if (action == null) {
            val prefs = getSharedPreferences(BootRecoveryReceiver.PREFERENCES, MODE_PRIVATE)
            if (prefs.getBoolean(KEY_WAS_RUNNING, false)) {
                promoteToForeground(
                    title = getString(R.string.app_name),
                    detail = "正在恢复节点与隧道 / restoring node & tunnel",
                )
                startEmbeddedControlAsync(autoStartNetworks = true)
                return START_STICKY
            }
            stopSelf()
            return START_NOT_STICKY
        }

        when (action) {
            ACTION_START_CONTROL -> {
                val restore = intent.getBooleanExtra(EXTRA_AUTO_RESTORE, true)
                promoteToForeground(
                    title = getString(R.string.app_name),
                    detail = "节点控制服务运行中 / control service active",
                )
                startEmbeddedControlAsync(autoStartNetworks = restore)
                return START_STICKY
            }
            TunnelActions.ACTION_START -> {
                startTunnel(intent)
                return START_STICKY
            }
            ACTION_STOP_DATA_PLANE -> {
                stopDataPlaneKeepControl()
                return START_STICKY
            }
            TunnelActions.ACTION_STOP -> {
                stopTunnelAndService()
                return START_NOT_STICKY
            }
            ACTION_NETWORK_CHANGED -> {
                worker.execute {
                    EmbeddedNodeHost.notifyNetworkPathChanged()
                    refreshNotification()
                }
                return START_STICKY
            }
            else -> return START_NOT_STICKY
        }
    }

    private fun startEmbeddedControlAsync(autoStartNetworks: Boolean) {
        val storageRoot = File(filesDir, "fuckipv6-node").absolutePath
        worker.execute {
            val started = EmbeddedNodeHost.ensureStarted(storageRoot, this)
            started.onSuccess { endpoint ->
                getSharedPreferences(BootRecoveryReceiver.PREFERENCES, MODE_PRIVATE)
                    .edit()
                    .putBoolean(KEY_WAS_RUNNING, true)
                    .apply()
                lastAlert = null
                if (autoStartNetworks && activeNetworks.isEmpty()) {
                    // Ask the authoritative embedded node to start any configured networks.
                    submitLocalCommand(endpoint, JSONObject().put("command", "start_node"))
                }
                refreshNotification()
            }.onFailure { error ->
                Log.w(TAG, "embedded runtime start failed: ${error.message}")
                lastAlert = error.message
                TunnelStateStore.apply(TunnelState.Failed("state.error.detail"))
                refreshNotification()
            }
        }
    }

    private fun startTunnel(intent: Intent) {
        val config = configFrom(intent)
        if (NativeNodeBridge.isLibraryLoaded) {
            preparedNetwork = config?.networkName ?: getString(R.string.app_name)
            promoteToForeground(
                title = preparedNetwork ?: getString(R.string.app_name),
                detail = "正在建立隧道 / preparing tunnel",
            )
            TunnelStateStore.apply(TunnelState.Preparing)
            val storageRoot = File(filesDir, "fuckipv6-node").absolutePath
            worker.execute {
                val started = EmbeddedNodeHost.ensureStarted(storageRoot, this)
                started.onSuccess { endpoint ->
                    getSharedPreferences(BootRecoveryReceiver.PREFERENCES, MODE_PRIVATE)
                        .edit()
                        .putBoolean(KEY_WAS_RUNNING, true)
                        .apply()
                    val targetNetwork = config?.networkName
                    val command = if (!targetNetwork.isNullOrBlank()) {
                        JSONObject()
                            .put("command", "start_networks")
                            .put(
                                "payload",
                                JSONObject().put("networks", org.json.JSONArray().put(targetNetwork)),
                            )
                    } else {
                        JSONObject().put("command", "start_node")
                    }
                    submitLocalCommand(endpoint, command)
                    if (activeNetworks.isNotEmpty()) {
                        TunnelStateStore.apply(TunnelState.Running)
                    }
                    refreshNotification()
                }.onFailure { error ->
                    lastAlert = error.message
                    finish(TunnelState.Failed("state.error.detail"))
                }
            }
            return
        }

        // Fallback pure TunnelEngine path (used when custom TunnelTransport is injected).
        if (config == null) {
            finish(TunnelState.Failed("state.error.detail"))
            return
        }
        preparedNetwork = config.networkName
        promoteToForeground(config.networkName, "正在建立隧道 / preparing tunnel")
        finish(engine.start(config))
    }

    /**
     * Stops only the data-plane networks (releasing TUN, routes and DNS) while
     * keeping the embedded loopback HTTP control service reachable (W16-02 #4).
     */
    private fun stopDataPlaneKeepControl() {
        TunnelStateStore.apply(TunnelState.Stopping)
        worker.execute {
            val endpoint = EmbeddedNodeHost.endpoint.value
            if (endpoint != null) {
                submitLocalCommand(endpoint, JSONObject().put("command", "stop_node"))
            }
            activeNetworks.clear()
            preparedNetwork = null
            TunnelStateStore.apply(TunnelState.Stopped)
            promoteToForeground(
                title = getString(R.string.app_name),
                detail = "数据面已停止，控制服务在线 / data plane stopped, control online",
            )
        }
    }

    private fun stopTunnelAndService() {
        getSharedPreferences(BootRecoveryReceiver.PREFERENCES, MODE_PRIVATE)
            .edit()
            .putBoolean(KEY_WAS_RUNNING, false)
            .apply()
        if (NativeNodeBridge.isLibraryLoaded && EmbeddedNodeHost.isRunning) {
            TunnelStateStore.apply(TunnelState.Stopping)
            worker.execute {
                EmbeddedNodeHost.stopAll()
                activeNetworks.clear()
                preparedNetwork = null
                TunnelStateStore.apply(TunnelState.Stopped)
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
            return
        }
        finish(engine.stop())
    }

    private fun submitLocalCommand(endpoint: EmbeddedNodeEndpoint, command: JSONObject) {
        runCatching {
            val client = NodeCommandClient(NodeApiConfig(endpoint.apiUrl, endpoint.token))
            val envelope = CommandJson.envelope(
                command = command,
                requestId = System.currentTimeMillis().coerceAtLeast(1L),
                idempotencyKey = "android-vpn-${UUID.randomUUID()}",
            )
            client.submit(envelope)
        }.onFailure { error ->
            Log.w(TAG, "local command failed: ${error.javaClass.simpleName}")
        }
    }

    /**
     * Called from Rust's `AndroidEmbeddedPlatform::open_network` on a blocking worker thread.
     * Establishes the Android TUN via `VpnService.Builder`, excludes this app package to prevent
     * routing loops, and hands the detached fd to Rust's `TunPump`.
     */
    override fun openNetwork(settingsJson: String): Int {
        val settings = NativeTunnelSettings.parse(settingsJson) ?: run {
            Log.w(TAG, "rejected invalid tunnel settings JSON from native node")
            return -1
        }
        val builder = Builder()
            .setSession(settings.network)
            .setMtu(settings.mtu)

        if (settings.disallowSelfPackage) {
            runCatching { builder.addDisallowedApplication(packageName) }
                .onFailure { error ->
                    Log.e(TAG, "addDisallowedApplication failed: ${error.javaClass.simpleName}")
                    return -2
                }
        }

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            builder.setMetered(false)
        }

        settings.ipv4Address?.let { addr ->
            builder.addAddress(addr, settings.ipv4Prefix)
        }
        settings.ipv6Address?.let { addr ->
            builder.addAddress(addr, settings.ipv6Prefix)
        }

        for (cidr in settings.routes) {
            val address = cidr.substringBefore('/')
            val prefix = cidr.substringAfter('/', "").toIntOrNull() ?: continue
            runCatching { builder.addRoute(address, prefix) }
                .onFailure { error ->
                    Log.w(TAG, "skipped invalid route $cidr: ${error.javaClass.simpleName}")
                }
        }
        for (dnsServer in settings.dns) {
            runCatching { builder.addDnsServer(dnsServer) }
                .onFailure { error ->
                    Log.w(TAG, "skipped invalid DNS $dnsServer: ${error.javaClass.simpleName}")
                }
        }

        val pfd = try {
            builder.establish()
        } catch (error: Exception) {
            Log.w(TAG, "VpnService.Builder.establish failed: ${error.javaClass.simpleName}")
            lastAlert = "vpn_consent_or_establish_failed"
            null
        } ?: return -3

        val detachedFd = pfd.detachFd()
        activeNetworks.add(settings.network)
        preparedNetwork = settings.network
        lastAlert = null
        TunnelStateStore.apply(TunnelState.Preparing)
        TunnelStateStore.apply(TunnelState.Running)
        refreshNotification()
        Log.i(
            TAG,
            "established Android TUN fd=$detachedFd net=${settings.network} mode=${settings.routingMode} routes=${settings.routes.size} dns=${settings.dns.size}",
        )
        return detachedFd
    }

    override fun closeNetwork(networkName: String) {
        activeNetworks.remove(networkName)
        if (activeNetworks.isEmpty()) {
            preparedNetwork = null
            if (TunnelStateStore.state.isActive) {
                TunnelStateStore.apply(TunnelState.Stopped)
            }
        }
        refreshNotification()
        Log.i(TAG, "closed Android TUN metadata for network=$networkName remaining=${activeNetworks.size}")
    }

    private fun finish(state: TunnelState) {
        when (state) {
            is TunnelState.Running -> refreshNotification()
            else -> {
                preparedNetwork = null
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
        }
    }

    override fun onRevoke() {
        Log.w(TAG, "OS revoked VPN permission; cleaning up TUN and data plane")
        lastAlert = "vpn_revoked"
        getSharedPreferences(BootRecoveryReceiver.PREFERENCES, MODE_PRIVATE)
            .edit()
            .putBoolean(KEY_WAS_RUNNING, false)
            .apply()
        if (NativeNodeBridge.isLibraryLoaded && EmbeddedNodeHost.isRunning) {
            worker.execute {
                EmbeddedNodeHost.stopAll()
                activeNetworks.clear()
                preparedNetwork = null
                TunnelStateStore.apply(TunnelState.Failed("state.offline.detail"))
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
        } else {
            finish(engine.revoked())
        }
        super.onRevoke()
    }

    override fun onDestroy() {
        recoveryObserver?.stop()
        recoveryObserver = null
        if (NativeNodeBridge.isLibraryLoaded && EmbeddedNodeHost.isRunning) {
            runCatching { EmbeddedNodeHost.stopAll() }
        }
        activeNetworks.clear()
        if (TunnelStateStore.state.isActive) {
            TunnelStateStore.apply(TunnelState.Stopped)
        }
        worker.shutdownNow()
        super.onDestroy()
    }

    /** Real TUN establishment for legacy/injected TunnelEngine path. */
    private fun establishInterface(config: TunnelConfig): TunnelInterfaceHandle? {
        val builder = Builder()
            .setSession(config.networkName)
            .setMtu(config.mtu)
            .addAddress(config.addressV4.substringBefore('/'), prefix(config.addressV4))
            .addRoute("0.0.0.0", 0)
        runCatching { builder.addDisallowedApplication(packageName) }
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

    private fun buildNotification(title: String, text: String): android.app.Notification {
        val openApp = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stopData = PendingIntent.getService(
            this,
            1,
            Intent(this, FuckIpv6VpnService::class.java).setAction(ACTION_STOP_DATA_PLANE),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val stopAll = PendingIntent.getService(
            this,
            2,
            Intent(this, FuckIpv6VpnService::class.java).setAction(TunnelActions.ACTION_STOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        return android.app.Notification.Builder(this, CHANNEL_ID)
            .setContentTitle(title)
            .setContentText(text)
            .setSmallIcon(R.drawable.ic_brand_mark)
            .setContentIntent(openApp)
            .addAction(
                android.app.Notification.Action.Builder(null, "暂停节点 / Pause", stopData).build(),
            )
            .addAction(
                android.app.Notification.Action.Builder(null, "停止 / Stop", stopAll).build(),
            )
            .setOngoing(true)
            .build()
    }

    private fun promoteToForeground(title: String, detail: String) {
        val notification = buildNotification(title, detail)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            startForeground(
                NOTIFICATION_ID,
                notification,
                android.content.pm.ServiceInfo.FOREGROUND_SERVICE_TYPE_SPECIAL_USE,
            )
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun refreshNotification() {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) !=
            android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val manager = getSystemService(NotificationManager::class.java) ?: return
        val title = preparedNetwork ?: getString(R.string.app_name)
        val telemetry = EmbeddedNodeHost.refreshTelemetry()
        val trafficSuffix = telemetry?.let {
            " · RX ${formatBytes(it.rxBytes)} / TX ${formatBytes(it.txBytes)}"
        }.orEmpty()
        val text = when {
            lastAlert != null -> "告警 / Alert: $lastAlert"
            activeNetworks.isNotEmpty() -> "隧道运行中 (${activeNetworks.size} 网络)$trafficSuffix"
            EmbeddedNodeHost.isRunning -> "控制服务在线 / control ready$trafficSuffix"
            else -> "已停止 / stopped"
        }
        runCatching { manager.notify(NOTIFICATION_ID, buildNotification(title, text)) }
    }

    private fun formatBytes(bytes: Long): String = when {
        bytes >= 1024L * 1024L -> String.format(java.util.Locale.US, "%.1f MiB", bytes / (1024.0 * 1024.0))
        bytes >= 1024L -> String.format(java.util.Locale.US, "%.1f KiB", bytes / 1024.0)
        else -> "$bytes B"
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

    /** Marker transport proving native JNI packet I/O is bound for [TunnelControlPlan]. */
    private class EmbeddedTunnelTransport(
        private val service: FuckIpv6VpnService,
    ) : TunnelTransport {
        override fun start(config: TunnelConfig, tun: ParcelFileDescriptor): Result<Unit> =
            Result.success(Unit)

        override fun stop() {
            service.activeNetworks.clear()
        }
    }

    companion object {
        const val CHANNEL_ID = "fuckipv6-tunnel"
        const val NOTIFICATION_ID = 41
        const val DEFAULT_MTU = 1400
        const val ACTION_START_CONTROL = "cc.xlei.fuckipv6.android.CONTROL_START"
        const val ACTION_STOP_DATA_PLANE = "cc.xlei.fuckipv6.android.DATA_PLANE_STOP"
        const val ACTION_NETWORK_CHANGED = "cc.xlei.fuckipv6.android.NETWORK_CHANGED"
        const val EXTRA_AUTO_RESTORE = "auto_restore"
        const val EXTRA_NETWORK_NAME = "network_name"
        const val EXTRA_ADDRESS_V4 = "address_v4"
        const val EXTRA_ADDRESS_V6 = "address_v6"
        const val EXTRA_MTU = "mtu"
        const val EXTRA_DNS = "dns"
        private const val KEY_WAS_RUNNING = "vpn_was_running"
        private const val TAG = "FuckIpv6Vpn"

        fun startControlIntent(context: Context, autoRestoreNetworks: Boolean = true): Intent =
            Intent(context, FuckIpv6VpnService::class.java)
                .setAction(ACTION_START_CONTROL)
                .putExtra(EXTRA_AUTO_RESTORE, autoRestoreNetworks)

        fun startIntent(context: Context, config: TunnelConfig): Intent =
            Intent(context, FuckIpv6VpnService::class.java)
                .setAction(TunnelActions.ACTION_START)
                .putExtra(EXTRA_NETWORK_NAME, config.networkName)
                .putExtra(EXTRA_ADDRESS_V4, config.addressV4)
                .putExtra(EXTRA_ADDRESS_V6, config.addressV6)
                .putExtra(EXTRA_MTU, config.mtu)
                .putStringArrayListExtra(EXTRA_DNS, ArrayList(config.dnsServers))

        fun stopDataPlaneIntent(context: Context): Intent =
            Intent(context, FuckIpv6VpnService::class.java)
                .setAction(ACTION_STOP_DATA_PLANE)

        fun stopIntent(context: Context): Intent =
            Intent(context, FuckIpv6VpnService::class.java)
                .setAction(TunnelActions.ACTION_STOP)

        fun networkChangedIntent(context: Context): Intent =
            Intent(context, FuckIpv6VpnService::class.java)
                .setAction(ACTION_NETWORK_CHANGED)
    }
}
