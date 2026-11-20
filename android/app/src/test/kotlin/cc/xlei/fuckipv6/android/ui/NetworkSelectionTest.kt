package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.NetworkLabel
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NetworkSelectionTest {
    private val networks = listOf(
        NetworkLabel("net-1", "家庭网络"),
        NetworkLabel("net-2", "工作网络"),
    )

    @Test
    fun selectionKeepsTheChosenNetwork() {
        assertEquals("net-2", NetworkSelection.resolve(networks, "工作网络")?.networkId)
        assertEquals("net-1", NetworkSelection.resolve(networks, "家庭网络")?.networkId)
    }

    @Test
    fun unknownOrMissingSelectionFallsBackToTheFirstEntry() {
        assertEquals("net-1", NetworkSelection.resolve(networks, "已删除的网络")?.networkId)
        assertEquals("net-1", NetworkSelection.resolve(networks, null)?.networkId)
        assertEquals(listOf("家庭网络", "工作网络"), NetworkSelection.names(networks))
    }

    @Test
    fun emptySnapshotHasNoSelection() {
        assertNull(NetworkSelection.resolve(emptyList(), "家庭网络"))
        assertEquals(listOf("家庭网络", "工作网络"), NetworkSelection.names(networks))
    }
}
