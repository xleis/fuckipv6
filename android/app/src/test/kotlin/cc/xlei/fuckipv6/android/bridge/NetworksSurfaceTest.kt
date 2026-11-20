package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NetworksSurfaceTest {
    private val snapshot = JSONObject(
        """
        {
          "networks": [
            {
              "name": "家庭网络",
              "tun_name": "fipv6-family",
              "status": "online",
              "assigned_ipv6": "fd00::2",
              "assigned_ipv4": "10.7.0.2",
              "p2p_listen_addr": "0.0.0.0:41641",
              "uptime_secs": 3600,
              "roles": ["member", "relay"],
              "peer_count": 3,
              "traffic": {"tun_rx_bytes": 100, "tun_tx_bytes": 200}
            },
            {"name": "工作网络"},
            {"tun_name": "no-name"},
            "not-an-object"
          ]
        }
        """.trimIndent(),
    )

    @Test
    fun visibleFieldsAreProjectedAsReported() {
        val networks = NetworksSurface.parse(snapshot)!!
        assertEquals(2, networks.size)
        val family = networks[0]
        assertEquals("家庭网络", family.name)
        assertEquals("fipv6-family", family.tunName)
        assertEquals("online", family.status)
        assertEquals("fd00::2", family.assignedIpv6)
        assertEquals("10.7.0.2", family.assignedIpv4)
        assertEquals("0.0.0.0:41641", family.listenAddr)
        assertEquals(3600L, family.uptimeSecs)
        assertEquals(listOf("member", "relay"), family.roles)
        assertEquals(3, family.peerCount)
        assertEquals(100L, family.traffic?.tunRxBytes)
    }

    @Test
    fun hiddenFieldsStayNullInsteadOfBecomingZero() {
        val work = NetworksSurface.parse(snapshot)!![1]
        assertEquals("工作网络", work.name)
        assertNull(work.tunName)
        assertNull(work.status)
        assertNull(work.assignedIpv6)
        assertNull(work.assignedIpv4)
        assertNull(work.listenAddr)
        assertNull(work.uptimeSecs)
        assertNull(work.roles)
        assertNull(work.peerCount)
        assertNull(work.traffic)
    }

    @Test
    fun malformedAndNamelessNetworksAreDropped() {
        assertEquals(2, NetworksSurface.parse(snapshot)!!.size)
        assertNull(NetworksSurface.parse(JSONObject("{}")))
        assertNull(NetworksSurface.parse(JSONObject("""{"networks":"nope"}""")))
        assertTrue(NetworksSurface.parse(JSONObject("""{"networks":[]}"""))!!.isEmpty())
    }

    @Test
    fun explicitlyEmptyRolesAreNotConfusedWithHiddenOnes() {
        val parsed = NetworksSurface.parse(JSONObject("""{"networks":[{"name":"n","roles":[]}]}"""))!!
        assertEquals(emptyList<String>(), parsed.single().roles)
    }
}
