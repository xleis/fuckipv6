package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PeersSurfaceTest {
    private val snapshot = JSONObject(
        """
        {
          "peers": [
            {
              "peer_id": "peer-a",
              "network_name": "家庭网络",
              "virtual_ipv6": "fd00::2",
              "virtual_ipv4": "10.7.0.2",
              "endpoints": [{"ip": "203.0.113.5", "port": 40000}],
              "online": true,
              "p2p_active": true,
              "p2p_healthy": true,
              "rtt_us": 4200,
              "loss_rate": 0.01
            },
            {"peer_id": "peer-b", "network_name": "工作网络", "online": false},
            {"peer_id": "", "network_name": "家庭网络"},
            "not-an-object"
          ]
        }
        """.trimIndent(),
    )

    @Test
    fun peersAreFilteredByNetworkAndKeepReachability() {
        val parsed = PeersSurface.parse(snapshot, "家庭网络")!!
        assertEquals("家庭网络", parsed.networkName)
        assertEquals(1, parsed.peers.size)
        val peer = parsed.peers.single()
        assertEquals("peer-a", peer.peerId)
        assertEquals("fd00::2", peer.virtualIpv6)
        assertEquals("10.7.0.2", peer.virtualIpv4)
        assertTrue(peer.online)
        assertTrue(peer.p2pActive)
        assertTrue(peer.p2pHealthy)
        assertEquals(4200L, peer.rttUs)
        assertEquals(0.01f, peer.lossRate!!, 0.0001f)
        assertEquals(1, peer.endpointCount)
        assertEquals(1, parsed.onlineCount)
    }

    @Test
    fun endpointAddressesNeverReachTheSurface() {
        val peer = PeersSurface.parse(snapshot, "家庭网络")!!.peers.single()
        assertFalse(peer.toString().contains("203.0.113.5"))
    }

    @Test
    fun missingPeersOrWrongTypeIsUnobservedNotEmpty() {
        assertNull(PeersSurface.parse(JSONObject("{}"), "家庭网络"))
        assertNull(PeersSurface.parse(JSONObject("""{"peers":"nope"}"""), "家庭网络"))
        val empty = PeersSurface.parse(JSONObject("""{"peers":[]}"""), "家庭网络")!!
        assertTrue(empty.peers.isEmpty())
        assertEquals(0, empty.onlineCount)
        val otherNetwork = PeersSurface.parse(snapshot, "不存在的网络")!!
        assertTrue(otherNetwork.peers.isEmpty())
    }

    @Test
    fun optionalFieldsStayAbsent() {
        val parsed = PeersSurface.parse(snapshot, "工作网络")!!
        val peer = parsed.peers.single()
        assertNull(peer.virtualIpv6)
        assertNull(peer.virtualIpv4)
        assertNull(peer.rttUs)
        assertNull(peer.lossRate)
        assertEquals(0, peer.endpointCount)
        assertFalse(peer.online)
    }
}
