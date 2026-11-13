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
        assertEquals(13, SurfaceStatus.entries.size)
    }
}
