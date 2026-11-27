package cc.xlei.fuckipv6.android.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.util.Log
import androidx.core.content.ContextCompat

/**
 * Restores the foreground control/VPN service after device reboot only when the
 * user explicitly opted in AND the OS VPN consent is already granted. Never
 * fakes consent or silently starts without user opt-in.
 */
class BootRecoveryReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent?) {
        val action = intent?.action ?: return
        if (action != Intent.ACTION_BOOT_COMPLETED && action != Intent.ACTION_MY_PACKAGE_REPLACED) {
            return
        }
        val prefs = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
        val autoRecover = prefs.getBoolean(KEY_AUTO_RECOVER_ON_BOOT, false)
        if (!autoRecover) {
            Log.i(TAG, "boot recovery skipped: auto-recover disabled by owner preference")
            return
        }
        val vpnPrepared = VpnService.prepare(context) == null
        val serviceIntent = if (vpnPrepared) {
            FuckIpv6VpnService.startControlIntent(context, autoRestoreNetworks = true)
        } else {
            Log.w(TAG, "VPN consent not granted after boot; starting control plane only")
            FuckIpv6VpnService.startControlIntent(context, autoRestoreNetworks = false)
        }
        runCatching {
            ContextCompat.startForegroundService(context, serviceIntent)
        }.onFailure { error ->
            Log.w(TAG, "boot recovery could not promote foreground service: ${error.javaClass.simpleName}")
        }
    }

    companion object {
        const val PREFERENCES = "fuckipv6.node"
        const val KEY_AUTO_RECOVER_ON_BOOT = "auto_recover_on_boot"
        private const val TAG = "FuckIpv6BootRecovery"
    }
}
