package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.contract.NodeContractV1.NodeFileCapabilitySnapshot
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeNetworkMembershipSnapshot
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class FileProjectionTest {
    private fun capability(
        state: String?,
        networkId: String? = null,
        accessEpoch: Long? = null,
        shareRevision: Long? = null,
        availableBytes: Long? = null,
        rootError: String? = null,
        activeTransfers: Long = 0,
    ) = NodeFileCapabilitySnapshot(
        accessEpoch,
        activeTransfers,
        availableBytes,
        networkId,
        rootError,
        shareRevision,
        state,
    )

    private fun membership(networkId: String, networkName: String) =
        NodeNetworkMembershipSnapshot(
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            networkId,
            networkName,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
            null,
        )

    @Test
    fun missingCapabilityRendersDisabledInsteadOfInventingState() {
        val data = FilePageData.from(null)
        assertEquals("status.capability.disabled", data.stateKey)
        assertEquals("neutral", data.stateTone)
        assertEquals(0, data.activeTransfers)
        assertNull(data.networkLabel)
        assertNull(data.formattedAvailable)
        assertFalse(data.showsTransferActions)
    }

    @Test
    fun enabledCapabilityKeepsEveryAuthoritativeField() {
        val data = FilePageData.from(
            capability(
                state = "enabled",
                networkId = "net-1",
                accessEpoch = 7,
                shareRevision = 12,
                availableBytes = 12L * 1024 * 1024 * 1024,
                activeTransfers = 2,
            ),
            listOf(membership("net-1", "家庭网络")),
        )
        assertEquals("status.capability.enabled", data.stateKey)
        assertEquals("good", data.stateTone)
        assertEquals("家庭网络", data.networkLabel)
        assertEquals(2, data.activeTransfers)
        assertEquals(7L, data.accessEpoch)
        assertEquals(12L, data.shareRevision)
        assertEquals("12.0 GiB", data.formattedAvailable)
        assertNull(data.rootErrorCode)
    }

    @Test
    fun degradedAndUnavailableStatesStayVisible() {
        val degraded = FilePageData.from(capability(state = "degraded"))
        assertEquals("status.capability.degraded", degraded.stateKey)
        assertEquals("warn", degraded.stateTone)

        val unavailable = FilePageData.from(
            capability(state = "unavailable", rootError = "file.error.root_missing"),
        )
        assertEquals("status.capability.unavailable", unavailable.stateKey)
        assertEquals("warn", unavailable.stateTone)
        assertEquals("file.error.root_missing", unavailable.rootErrorCode)
    }

    @Test
    fun unrecognisedWireStateNeverLooksUsable() {
        val data = FilePageData.from(capability(state = "ENABLED_MAYBE"))
        assertEquals("status.capability.unknown", data.stateKey)
        assertEquals("neutral", data.stateTone)
    }

    @Test
    fun ownerLocalProseIsReducedToAStableCode() {
        assertEquals(
            "file.error.root_missing",
            FilePageData.stableErrorCode("file.error.root_missing"),
        )
        assertEquals(
            "file.error.unspecified",
            FilePageData.stableErrorCode("/home/alice/共享 目录不存在"),
        )
        assertEquals(
            "file.error.unspecified",
            FilePageData.stableErrorCode("error: EACCES reading /srv/share"),
        )
    }

    @Test
    fun byteFormattingIsDeterministic() {
        assertEquals("0 B", FilePageData.formatBytes(0))
        assertEquals("512 B", FilePageData.formatBytes(512))
        assertEquals("1.0 KiB", FilePageData.formatBytes(1024))
        assertEquals("1.5 MiB", FilePageData.formatBytes(1024L * 1024 + 512 * 1024))
        assertEquals("2.0 GiB", FilePageData.formatBytes(2L * 1024 * 1024 * 1024))
        assertEquals("--", FilePageData.formatBytes(-1))
    }

    @Test
    fun transferSurfaceStaysOnDesktopSurfaces() {
        val data = FilePageData.from(capability(state = "enabled"))
        assertEquals("file.page.local_state.note", data.transferSurfaceKey)
        assertFalse(data.showsTransferActions)
        assertTrue(data.formattedAvailable == null)
    }
}
