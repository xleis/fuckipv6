package cc.xlei.fuckipv6.android.bridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class NetworkWorkflowSurfaceTest {
    @Test
    fun encodesCreateJoinDiscoveryAndLifecycleCommands() {
        assertEquals("create_network", NetworkWorkflowSurface.createNetworkCommand("hk-mesh")!!.getString("command"))
        assertNull(NetworkWorkflowSurface.createNetworkCommand(" "))

        assertEquals("join_network", NetworkWorkflowSurface.joinNetworkCommand("fv6://ticket")!!.getString("command"))
        assertEquals("preview_join_network", NetworkWorkflowSurface.previewJoinNetworkCommand("net_1")!!.getString("command"))
        assertEquals("preview_invitation", NetworkWorkflowSurface.previewInvitationCommand("net_1", "inv_code")!!.getString("command"))
        assertEquals("redeem_invitation", NetworkWorkflowSurface.redeemInvitationCommand("net_1", "inv_code")!!.getString("command"))
        assertEquals("refresh_discovery_index", NetworkWorkflowSurface.refreshDiscoveryIndexCommand("hk").getString("command"))
        assertEquals("retry_discovery_index", NetworkWorkflowSurface.retryDiscoveryIndexCommand().getString("command"))
        assertEquals("start_node", NetworkWorkflowSurface.startNodeCommand().getString("command"))
        assertEquals("stop_node", NetworkWorkflowSurface.stopNodeCommand().getString("command"))
        assertEquals("start_networks", NetworkWorkflowSurface.startNetworksCommand(listOf("hk")).getString("command"))
        assertEquals("stop_networks", NetworkWorkflowSurface.stopNetworksCommand(listOf("hk")).getString("command"))
        assertEquals("restart_networks", NetworkWorkflowSurface.restartNetworksCommand(listOf("hk")).getString("command"))
        assertEquals("rebuild_tun", NetworkWorkflowSurface.rebuildTunCommand("hk")!!.getString("command"))
        assertEquals("update_coordinator_candidates", NetworkWorkflowSurface.updateCoordinatorCandidatesCommand("hk", true, 1L)!!.getString("command"))
    }
}
