package cc.xlei.fuckipv6.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class SurfaceStateTest {
    @Test
    fun everySurfaceStatusMapsToSharedTranslationKeys() {
        SurfaceStatus.entries.forEach { status ->
            val state = SurfaceState(status)
            assertEquals("state.${status.wireName}.title", state.titleKey)
            assertEquals("state.${status.wireName}.detail", state.detailKey)
        }
        assertEquals(14, SurfaceStatus.entries.size)
        assertEquals(
            SurfaceStatus.NOT_AVAILABLE_ON_DEVICE,
            SurfaceStatus.entries.first { it.wireName == "not_available_on_device" },
        )
    }

    /** A page this device does not own must not borrow the control-plane wording. */
    @Test
    fun unsupportedPagesCarryTheirOwnDeviceScopeCopy() {
        val state = SurfaceState(SurfaceStatus.NOT_AVAILABLE_ON_DEVICE)

        assertEquals("state.not_available_on_device.title", state.titleKey)
        assertEquals("state.not_available_on_device.detail", state.detailKey)
    }

    /** A failed fetch names its own cause instead of blaming the control service. */
    @Test
    fun bridgeFailuresMoveTheHeadlineToTheRealCause() {
        assertEquals(
            SurfaceStatus.PERMISSION_DENIED,
            failureStatus("state.permission_denied.detail"),
        )
        assertEquals(SurfaceStatus.OFFLINE, failureStatus("state.offline.detail"))
        assertEquals(SurfaceStatus.ERROR, failureStatus("state.error.detail"))
        assertEquals(SurfaceStatus.UNSUPPORTED, failureStatus("state.unsupported.detail"))
        assertEquals(SurfaceStatus.CONTROL_UNAVAILABLE, failureStatus(null))
        assertEquals(SurfaceStatus.CONTROL_UNAVAILABLE, failureStatus("bridge.error.other"))
    }

    @Test
    fun detailOverrideWinsOverTheStatusDetail() {
        val state = SurfaceState(
            SurfaceStatus.OFFLINE,
            detailKeyOverride = "state.error.detail",
        )

        assertEquals("state.offline.title", state.titleKey)
        assertEquals("state.error.detail", state.detailKey)
    }
}