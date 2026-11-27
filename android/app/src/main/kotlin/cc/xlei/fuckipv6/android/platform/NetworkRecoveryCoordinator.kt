package cc.xlei.fuckipv6.android.platform

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.LinkProperties
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.PowerManager
import android.util.Log
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Bounded Android network-switch, Doze, sleep/wake and airplane-mode recovery
 * coordinator (W16-07).
 *
 * Guarantees:
 * - Hard recovery timeout (`HARD_RECOVERY_DEADLINE_MS = 10_000L`) and bounded
 *   attempt count (`MAX_CONSECUTIVE_ATTEMPTS = 3`) so the app never loops
 *   indefinitely or retains stale notifications.
 * - On unrecoverable failure or airplane mode, transitions cleanly to a visible
 *   degraded/stopped state and triggers cleanup.
 */
enum class RecoveryPhase {
    IDLE,
    RECOVERING,
    DEGRADED,
    STOPPED_OFFLINE,
}

enum class RecoveryEventKind {
    WIFI_TO_CELLULAR,
    CELLULAR_TO_WIFI,
    LINK_ADDRESS_CHANGED,
    DOZE_ENTERED,
    DOZE_EXITED,
    SCREEN_WAKE,
    AIRPLANE_MODE_ON,
    AIRPLANE_MODE_OFF,
    NETWORK_LOST,
}

data class RecoveryStatus(
    val phase: RecoveryPhase = RecoveryPhase.IDLE,
    val attempts: Int = 0,
    val lastEvent: RecoveryEventKind? = null,
    val lastReason: String? = null,
    val deadlineUnixMs: Long? = null,
    val lastRecoveredAtUnixMs: Long? = null,
)

enum class RecoveryDirective {
    NONE,
    TRIGGER_PATH_RECONNECT,
    CLEANUP_AND_DEGRADE,
    CLEANUP_AND_STOP,
}

class BoundedRecoveryStateMachine(
    private val maxAttempts: Int = MAX_CONSECUTIVE_ATTEMPTS,
    private val hardDeadlineMs: Long = HARD_RECOVERY_DEADLINE_MS,
) {
    private var status = RecoveryStatus()

    fun current(): RecoveryStatus = status

    fun onEvent(event: RecoveryEventKind, nowUnixMs: Long): RecoveryDirective {
        return when (event) {
            RecoveryEventKind.AIRPLANE_MODE_ON -> {
                status = RecoveryStatus(
                    phase = RecoveryPhase.STOPPED_OFFLINE,
                    attempts = 0,
                    lastEvent = event,
                    lastReason = "airplane_mode",
                    deadlineUnixMs = null,
                )
                RecoveryDirective.CLEANUP_AND_STOP
            }
            RecoveryEventKind.DOZE_ENTERED -> {
                status = status.copy(
                    phase = RecoveryPhase.DEGRADED,
                    lastEvent = event,
                    lastReason = "device_doze_idle",
                )
                RecoveryDirective.NONE
            }
            RecoveryEventKind.NETWORK_LOST -> {
                val nextAttempts = status.attempts + 1
                if (nextAttempts > maxAttempts) {
                    status = RecoveryStatus(
                        phase = RecoveryPhase.DEGRADED,
                        attempts = nextAttempts,
                        lastEvent = event,
                        lastReason = "recovery_attempts_exhausted",
                        deadlineUnixMs = null,
                    )
                    RecoveryDirective.CLEANUP_AND_DEGRADE
                } else {
                    status = RecoveryStatus(
                        phase = RecoveryPhase.RECOVERING,
                        attempts = nextAttempts,
                        lastEvent = event,
                        lastReason = "network_lost_waiting_path",
                        deadlineUnixMs = nowUnixMs + hardDeadlineMs,
                    )
                    RecoveryDirective.NONE
                }
            }
            RecoveryEventKind.WIFI_TO_CELLULAR,
            RecoveryEventKind.CELLULAR_TO_WIFI,
            RecoveryEventKind.LINK_ADDRESS_CHANGED,
            RecoveryEventKind.DOZE_EXITED,
            RecoveryEventKind.SCREEN_WAKE,
            RecoveryEventKind.AIRPLANE_MODE_OFF,
            -> {
                val nextAttempts = if (status.phase == RecoveryPhase.RECOVERING) status.attempts + 1 else 1
                if (nextAttempts > maxAttempts) {
                    status = RecoveryStatus(
                        phase = RecoveryPhase.DEGRADED,
                        attempts = nextAttempts,
                        lastEvent = event,
                        lastReason = "recovery_attempts_exhausted",
                        deadlineUnixMs = null,
                    )
                    RecoveryDirective.CLEANUP_AND_DEGRADE
                } else {
                    status = RecoveryStatus(
                        phase = RecoveryPhase.RECOVERING,
                        attempts = nextAttempts,
                        lastEvent = event,
                        lastReason = event.name.lowercase(),
                        deadlineUnixMs = nowUnixMs + hardDeadlineMs,
                    )
                    RecoveryDirective.TRIGGER_PATH_RECONNECT
                }
            }
        }
    }

    fun onTick(nowUnixMs: Long): RecoveryDirective {
        val deadline = status.deadlineUnixMs ?: return RecoveryDirective.NONE
        if (status.phase == RecoveryPhase.RECOVERING && nowUnixMs >= deadline) {
            status = status.copy(
                phase = RecoveryPhase.DEGRADED,
                lastReason = "recovery_hard_timeout",
                deadlineUnixMs = null,
            )
            return RecoveryDirective.CLEANUP_AND_DEGRADE
        }
        return RecoveryDirective.NONE
    }

    fun onRecoverySucceeded(nowUnixMs: Long) {
        status = RecoveryStatus(
            phase = RecoveryPhase.IDLE,
            attempts = 0,
            lastEvent = status.lastEvent,
            lastReason = "recovered",
            deadlineUnixMs = null,
            lastRecoveredAtUnixMs = nowUnixMs,
        )
    }

    fun onRecoveryFailed(reason: String) {
        status = status.copy(
            phase = RecoveryPhase.DEGRADED,
            lastReason = reason,
            deadlineUnixMs = null,
        )
    }

    companion object {
        const val MAX_CONSECUTIVE_ATTEMPTS: Int = 3
        const val HARD_RECOVERY_DEADLINE_MS: Long = 10_000L
    }
}

class AndroidNetworkRecoveryObserver(
    private val context: Context,
    private val onPathReconnect: () -> Boolean,
    private val onCleanupRequired: (stopCompletely: Boolean, reason: String) -> Unit,
) {
    private val machine = BoundedRecoveryStateMachine()
    private val statusFlow = MutableStateFlow(machine.current())
    val status: StateFlow<RecoveryStatus> = statusFlow.asStateFlow()

    private var registered = false
    private var lastTransport: String? = null
    private var lastAddressesSignature: String? = null

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
            val current = when {
                caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
                caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
                else -> "other"
            }
            val prev = lastTransport
            lastTransport = current
            if (prev != null && prev != current) {
                val event = if (current == "cellular") {
                    RecoveryEventKind.WIFI_TO_CELLULAR
                } else {
                    RecoveryEventKind.CELLULAR_TO_WIFI
                }
                dispatch(event)
            }
        }

        override fun onLinkPropertiesChanged(network: Network, linkProperties: LinkProperties) {
            val sig = linkProperties.linkAddresses
                .map { it.address.hostAddress.orEmpty() }
                .sorted()
                .joinToString(",")
            val prev = lastAddressesSignature
            lastAddressesSignature = sig
            if (prev != null && prev != sig && sig.isNotEmpty()) {
                dispatch(RecoveryEventKind.LINK_ADDRESS_CHANGED)
            }
        }

        override fun onLost(network: Network) {
            dispatch(RecoveryEventKind.NETWORK_LOST)
        }
    }

    private val powerReceiver = object : BroadcastReceiver() {
        override fun onReceive(ctx: Context, intent: Intent?) {
            when (intent?.action) {
                PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED -> {
                    val pm = ctx.getSystemService(PowerManager::class.java)
                    val idle = runCatching { pm?.isDeviceIdleMode == true }.getOrDefault(false)
                    dispatch(if (idle) RecoveryEventKind.DOZE_ENTERED else RecoveryEventKind.DOZE_EXITED)
                }
                Intent.ACTION_SCREEN_ON -> dispatch(RecoveryEventKind.SCREEN_WAKE)
                Intent.ACTION_AIRPLANE_MODE_CHANGED -> {
                    val on = intent.getBooleanExtra("state", false)
                    dispatch(if (on) RecoveryEventKind.AIRPLANE_MODE_ON else RecoveryEventKind.AIRPLANE_MODE_OFF)
                }
            }
        }
    }

    fun start() {
        if (registered) return
        registered = true
        val cm = context.getSystemService(ConnectivityManager::class.java)
        runCatching { cm?.registerDefaultNetworkCallback(networkCallback) }
            .onFailure { Log.w(TAG, "registerDefaultNetworkCallback failed: ${it.javaClass.simpleName}") }
        val filter = IntentFilter().apply {
            addAction(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED)
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_AIRPLANE_MODE_CHANGED)
        }
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                context.registerReceiver(powerReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
            } else {
                context.registerReceiver(powerReceiver, filter)
            }
        }.onFailure { Log.w(TAG, "registerReceiver failed: ${it.javaClass.simpleName}") }
    }

    fun stop() {
        if (!registered) return
        registered = false
        val cm = context.getSystemService(ConnectivityManager::class.java)
        runCatching { cm?.unregisterNetworkCallback(networkCallback) }
        runCatching { context.unregisterReceiver(powerReceiver) }
    }

    private fun dispatch(event: RecoveryEventKind) {
        val now = System.currentTimeMillis()
        val directive = machine.onEvent(event, now)
        statusFlow.value = machine.current()
        when (directive) {
            RecoveryDirective.NONE -> Unit
            RecoveryDirective.TRIGGER_PATH_RECONNECT -> {
                val ok = runCatching { onPathReconnect() }.getOrDefault(false)
                if (ok) {
                    machine.onRecoverySucceeded(System.currentTimeMillis())
                } else {
                    machine.onRecoveryFailed("path_reconnect_failed")
                    onCleanupRequired(false, "path_reconnect_failed")
                }
                statusFlow.value = machine.current()
            }
            RecoveryDirective.CLEANUP_AND_DEGRADE -> {
                onCleanupRequired(false, machine.current().lastReason ?: "degraded")
            }
            RecoveryDirective.CLEANUP_AND_STOP -> {
                onCleanupRequired(true, machine.current().lastReason ?: "stopped_offline")
            }
        }
    }

    companion object {
        private const val TAG = "FuckIpv6Recovery"
    }
}
