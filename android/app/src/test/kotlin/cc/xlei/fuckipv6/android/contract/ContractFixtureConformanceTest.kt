package cc.xlei.fuckipv6.android.contract

import cc.xlei.fuckipv6.android.bridge.ActivitySurface
import cc.xlei.fuckipv6.android.bridge.ExitSurface
import cc.xlei.fuckipv6.android.bridge.IdentitySurface
import cc.xlei.fuckipv6.android.bridge.LogsSurface
import cc.xlei.fuckipv6.android.bridge.NetworksSurface
import cc.xlei.fuckipv6.android.bridge.PeersSurface
import cc.xlei.fuckipv6.android.bridge.RoutingSurface
import cc.xlei.fuckipv6.android.bridge.SnapshotJsonParser
import cc.xlei.fuckipv6.android.bridge.TopologySurface
import cc.xlei.fuckipv6.android.bridge.TrafficSurface
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Reads the shipped visual-regression fixture with the production parsers.
 *
 * The desktop surfaces render this exact artifact, so parsing it here proves
 * Android reads the same snapshot shape instead of a hand-written imitation. It
 * also proves the parsers stay honest about slices the fixture does not carry:
 * absent topology/routing/activity must be `null`, never empty-but-present.
 */
class ContractFixtureConformanceTest {
    private val fixture: JSONObject by lazy {
        val text = javaClass.classLoader!!
            .getResourceAsStream("product-visual-snapshot.json")!!
            .bufferedReader()
            .use { it.readText() }
        JSONObject(text)
    }

    private val snapshot: JSONObject by lazy { fixture.getJSONObject("snapshot") }

    @Test
    fun theFileCapabilitySliceParsesThroughTheSnapshotParser() {
        val parsed = SnapshotJsonParser.parseSnapshot("""{"snapshot":${snapshot}}""")
        assertTrue(parsed.isSuccess)
        val view = parsed.getOrThrow()
        assertEquals(4242L, view.file.snapshotRevision)
    }

    @Test
    fun identityAndNetworksComeFromTheFixtureVerbatim() {
        val identity = IdentitySurface.parse(snapshot)!!
        assertEquals("node-visual-01", identity.nodeId)
        assertEquals("Visual Fixture Node", identity.displayName)
        assertTrue(identity.fingerprint!!.startsWith("SHA256:"))

        val networks = NetworksSurface.parse(snapshot)!!
        assertEquals(listOf("home", "studio"), networks.map { it.name })
        assertEquals(listOf("member", "exit_node"), networks[0].roles)
        assertEquals("fd00::2", networks[0].assignedIpv6)
        assertNull(networks[1].assignedIpv4)
        assertEquals(2, networks[0].peerCount)
    }

    @Test
    fun peersAndTrafficAreDerivedPerNetwork() {
        val home = PeersSurface.parse(snapshot, "home")!!
        assertEquals(2, home.peers.size)
        assertEquals(2, home.onlineCount)
        assertTrue(home.peers.all { it.endpointCount == 1 })

        val studio = PeersSurface.parse(snapshot, "studio")!!
        assertTrue(studio.peers.isEmpty())

        val traffic = TrafficSurface.parse(snapshot, "home")!!
        assertTrue(traffic.tunRxBytes > 0)
        assertEquals(traffic.tunRxBytes, NetworksSurface.parse(snapshot)!![0].traffic!!.tunRxBytes)
    }

    @Test
    fun logsAreOwnerVisibleInTheFixture() {
        val logs = LogsSurface.parse(snapshot)!!
        assertTrue(logs.hasLines)
        assertEquals(5, logs.lines.size)
        assertEquals("info", logs.lines.first().level)
        assertEquals("node.lifecycle", logs.lines.first().target)
    }

    @Test
    fun slicesTheFixtureDoesNotCarryStayUnobserved() {
        assertNull(TopologySurface.parse(snapshot))
        assertNull(RoutingSurface.parse(snapshot))
        assertNull(ActivitySurface.parse(snapshot, null))
        assertNull(ExitSurface.parseLocalExit(snapshot))
        assertNull(ExitSurface.parseSelections(snapshot))
        assertNotNull(snapshot.opt("revision"))
        assertNull(snapshot.opt("file_capability"))
    }
}
