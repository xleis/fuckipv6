package cc.xlei.fuckipv6.android.platform

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DevicePolicyMonitorTest {
    private val baseEnv = DeviceEnvironmentSnapshot(
        ignoringBatteryOptimizations = true,
        dozeIdleMode = false,
        powerSaveMode = false,
        batteryPercent = 80,
        isCharging = false,
        thermalStatus = 0,
        isMetered = false,
        isRoaming = false,
        transportKind = "wifi",
        manufacturer = "samsung",
        androidSdk = 36,
    )

    @Test
    fun normalWifiWithBatteryExemptionAllowsFullServices() {
        val decision = DevicePolicyEvaluator.evaluate(baseEnv, MeteredPolicySettings())
        assertEquals(DegradationLevel.NORMAL, decision.degradation)
        assertTrue(decision.allowBackgroundHeavyServices)
        assertTrue(decision.allowMeteredDataPlane)
        assertEquals("samsung_one_ui_sleeping_apps", decision.vendorHintKey)
        assertTrue(decision.warnings.isEmpty())
    }

    @Test
    fun meteredAndRoamingRestrictHeavyBackgroundServicesByDefault() {
        val decision = DevicePolicyEvaluator.evaluate(
            baseEnv.copy(isMetered = true, isRoaming = true, transportKind = "cellular"),
            MeteredPolicySettings(),
        )
        assertEquals(DegradationLevel.METERED_CONSERVATIVE, decision.degradation)
        assertFalse(decision.allowBackgroundHeavyServices)
        assertFalse(decision.allowMeteredDataPlane)
        assertTrue("roaming_restricted" in decision.warnings)
    }

    @Test
    fun lowBatteryAndThermalPressureDegradeAutomatically() {
        val lowBat = DevicePolicyEvaluator.evaluate(
            baseEnv.copy(batteryPercent = 12, isCharging = false),
            MeteredPolicySettings(),
        )
        assertEquals(DegradationLevel.LOW_BATTERY_DEGRADED, lowBat.degradation)
        assertFalse(lowBat.allowBackgroundHeavyServices)

        val hot = DevicePolicyEvaluator.evaluate(
            baseEnv.copy(thermalStatus = DevicePolicyEvaluator.THERMAL_SEVERE_LEVEL),
            MeteredPolicySettings(),
        )
        assertEquals(DegradationLevel.THERMAL_DEGRADED, hot.degradation)
        assertFalse(hot.allowBackgroundHeavyServices)
    }

    @Test
    fun unexemptBatteryOptimizationIsReportedHonestly() {
        val decision = DevicePolicyEvaluator.evaluate(
            baseEnv.copy(ignoringBatteryOptimizations = false, dozeIdleMode = true),
            MeteredPolicySettings(),
        )
        assertTrue("battery_optimization_active" in decision.warnings)
        assertTrue("doze_idle_active" in decision.warnings)
    }
}
