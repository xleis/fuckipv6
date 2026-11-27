package cc.xlei.fuckipv6.android.platform

import android.content.Context
import android.content.Intent
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.os.BatteryManager
import android.os.Build
import android.os.PowerManager
import android.provider.Settings
import java.util.Locale

/**
 * Background restriction, battery, thermal and metered-network policy monitor
 * (W16-06).
 *
 * Never fakes battery-optimization exemption or hides background restrictions.
 */
enum class DegradationLevel {
    NORMAL,
    METERED_CONSERVATIVE,
    LOW_BATTERY_DEGRADED,
    THERMAL_DEGRADED,
}

data class DeviceEnvironmentSnapshot(
    val ignoringBatteryOptimizations: Boolean,
    val dozeIdleMode: Boolean,
    val powerSaveMode: Boolean,
    val batteryPercent: Int,
    val isCharging: Boolean,
    val thermalStatus: Int,
    val isMetered: Boolean,
    val isRoaming: Boolean,
    val transportKind: String,
    val manufacturer: String,
    val androidSdk: Int,
)

data class DevicePolicyDecision(
    val degradation: DegradationLevel,
    val allowBackgroundHeavyServices: Boolean,
    val allowMeteredDataPlane: Boolean,
    val meteredQuotaBytes: Long,
    val warnings: List<String>,
    val vendorHintKey: String,
)

data class MeteredPolicySettings(
    val allowHeavyServicesOnMetered: Boolean = false,
    val allowRoamingDataPlane: Boolean = false,
    val meteredQuotaMb: Long = DEFAULT_METERED_QUOTA_MB,
    val degradeOnLowBattery: Boolean = true,
) {
    companion object {
        const val DEFAULT_METERED_QUOTA_MB: Long = 512L
    }
}

object DevicePolicyEvaluator {
    const val LOW_BATTERY_THRESHOLD: Int = 15
    const val THERMAL_SEVERE_LEVEL: Int = 3 // PowerManager.THERMAL_STATUS_SEVERE

    fun evaluate(
        env: DeviceEnvironmentSnapshot,
        settings: MeteredPolicySettings,
        sessionBytesUsed: Long = 0L,
    ): DevicePolicyDecision {
        val warnings = mutableListOf<String>()
        if (!env.ignoringBatteryOptimizations) {
            warnings += "battery_optimization_active"
        }
        if (env.dozeIdleMode) {
            warnings += "doze_idle_active"
        }
        if (env.powerSaveMode) {
            warnings += "power_save_active"
        }

        val quotaBytes = settings.meteredQuotaMb.coerceAtLeast(16L) * 1024L * 1024L
        val quotaExceeded = env.isMetered && sessionBytesUsed >= quotaBytes
        if (quotaExceeded) {
            warnings += "metered_quota_exceeded"
        }
        if (env.isRoaming && !settings.allowRoamingDataPlane) {
            warnings += "roaming_restricted"
        }

        val thermalDegraded = env.thermalStatus >= THERMAL_SEVERE_LEVEL
        val batteryDegraded = settings.degradeOnLowBattery && !env.isCharging &&
            env.batteryPercent in 0..LOW_BATTERY_THRESHOLD
        val meteredRestricted = (env.isMetered && !settings.allowHeavyServicesOnMetered) ||
            (env.isRoaming && !settings.allowRoamingDataPlane) ||
            quotaExceeded

        val degradation = when {
            thermalDegraded -> {
                warnings += "thermal_throttling"
                DegradationLevel.THERMAL_DEGRADED
            }
            batteryDegraded -> {
                warnings += "low_battery_degraded"
                DegradationLevel.LOW_BATTERY_DEGRADED
            }
            meteredRestricted -> DegradationLevel.METERED_CONSERVATIVE
            else -> DegradationLevel.NORMAL
        }

        val allowHeavy = degradation == DegradationLevel.NORMAL && !env.powerSaveMode
        val allowMeteredData = !quotaExceeded && (!env.isRoaming || settings.allowRoamingDataPlane)

        return DevicePolicyDecision(
            degradation = degradation,
            allowBackgroundHeavyServices = allowHeavy,
            allowMeteredDataPlane = allowMeteredData,
            meteredQuotaBytes = quotaBytes,
            warnings = warnings,
            vendorHintKey = vendorBackgroundHintKey(env.manufacturer),
        )
    }

    fun vendorBackgroundHintKey(manufacturer: String): String {
        val m = manufacturer.lowercase(Locale.ROOT)
        return when {
            m.contains("samsung") -> "samsung_one_ui_sleeping_apps"
            m.contains("xiaomi") || m.contains("redmi") || m.contains("poco") -> "miui_autostart_battery_saver"
            m.contains("huawei") || m.contains("honor") -> "emui_app_launch_management"
            m.contains("oppo") || m.contains("realme") || m.contains("oneplus") -> "coloros_background_freeze"
            m.contains("vivo") || m.contains("iqoo") -> "originos_high_background_power"
            else -> "aosp_doze_app_standby"
        }
    }
}

class AndroidDevicePolicyMonitor(private val context: Context) {
    fun captureSnapshot(): DeviceEnvironmentSnapshot {
        val pm = context.getSystemService(PowerManager::class.java)
        val cm = context.getSystemService(ConnectivityManager::class.java)
        val bm = context.getSystemService(BatteryManager::class.java)

        val ignoringBattery = runCatching {
            pm?.isIgnoringBatteryOptimizations(context.packageName) == true
        }.getOrDefault(false)
        val doze = runCatching { pm?.isDeviceIdleMode == true }.getOrDefault(false)
        val powerSave = runCatching { pm?.isPowerSaveMode == true }.getOrDefault(false)
        val thermal = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            runCatching { pm?.currentThermalStatus ?: 0 }.getOrDefault(0)
        } else {
            0
        }
        val batteryPct = runCatching {
            bm?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: -1
        }.getOrDefault(-1)
        val charging = runCatching { bm?.isCharging == true }.getOrDefault(false)

        val activeNet = runCatching { cm?.activeNetwork }.getOrNull()
        val caps = activeNet?.let { runCatching { cm?.getNetworkCapabilities(it) }.getOrNull() }
        val metered = runCatching { cm?.isActiveNetworkMetered == true }.getOrDefault(false)
        val roaming = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P && caps != null) {
            !caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_NOT_ROAMING)
        } else {
            false
        }
        val transportKind = when {
            caps == null -> "offline"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) -> "wifi"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_CELLULAR) -> "cellular"
            caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET) -> "ethernet"
            else -> "other"
        }

        return DeviceEnvironmentSnapshot(
            ignoringBatteryOptimizations = ignoringBattery,
            dozeIdleMode = doze,
            powerSaveMode = powerSave,
            batteryPercent = batteryPct,
            isCharging = charging,
            thermalStatus = thermal,
            isMetered = metered,
            isRoaming = roaming,
            transportKind = transportKind,
            manufacturer = Build.MANUFACTURER.orEmpty(),
            androidSdk = Build.VERSION.SDK_INT,
        )
    }

    /**
     * Opens the system battery-optimization settings page so the operator can
     * decide whether to exempt FuckIPv6. Never claims exemption was granted
     * until [captureSnapshot] observes `ignoringBatteryOptimizations == true`.
     */
    fun batteryOptimizationSettingsIntent(): Intent =
        Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
}
