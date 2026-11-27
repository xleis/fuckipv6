package cc.xlei.fuckipv6.android.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NativeNodeBridgeTest {
    @Test
    fun startResultParsesValidLoopbackEndpointAndRejectsErrors() {
        val ok = NativeNodeBridge.parseStartResult(
            """{"ok":true,"handle":42,"api_url":"http://127.0.0.1:43210","token":"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef","node_id":"node-abc"}""",
        )
        assertTrue(ok.isSuccess)
        val parsed = ok.getOrThrow()
        assertEquals(42L, parsed.handle)
        assertEquals("http://127.0.0.1:43210", parsed.apiUrl)
        assertEquals("node-abc", parsed.nodeId)

        val err = NativeNodeBridge.parseStartResult("""{"ok":false,"code":"unavailable","message":"locked"}""")
        assertTrue(err.isFailure)

        val nonLoopback = NativeNodeBridge.parseStartResult(
            """{"ok":true,"handle":1,"api_url":"http://10.0.0.1:8080","token":"0123456789abcdef0123456789abcdef"}""",
        )
        assertTrue(nonLoopback.isFailure)
    }

    @Test
    fun telemetryAndNativeTunnelSettingsParseCleanly() {
        val telemetry = NativeNodeBridge.parseTelemetry(
            """{"uptime_secs":95,"active_tunnel_sessions":1,"counters":{"rx_packets":10,"rx_bytes":2048,"tx_packets":12,"tx_bytes":4096,"dropped_packets":1}}""",
        )
        assertNotNull(telemetry)
        assertEquals(95L, telemetry!!.uptimeSecs)
        assertEquals(1, telemetry.activeTunnelSessions)
        assertEquals(2048L, telemetry.rxBytes)
        assertEquals(4096L, telemetry.txBytes)

        val settings = NativeTunnelSettings.parse(
            """{"network":"hk-exit","mtu":1400,"routing_mode":"global_proxy","ipv4":{"address":"10.8.0.5","prefix":24},"ipv6":{"address":"fd00::5","prefix":64},"routes":["0.0.0.0/0","::/0"],"dns":["1.1.1.1","8.8.8.8"],"disallow_self_package":true}""",
        )
        assertNotNull(settings)
        assertEquals("hk-exit", settings!!.network)
        assertEquals("global_proxy", settings.routingMode)
        assertEquals(listOf("0.0.0.0/0", "::/0"), settings.routes)
        assertEquals(listOf("1.1.1.1", "8.8.8.8"), settings.dns)
        assertTrue(settings.disallowSelfPackage)

        assertNull(NativeTunnelSettings.parse("""{"network":"","mtu":1400}"""))
        assertNull(NativeTunnelSettings.parse("""{"network":"x","mtu":500}"""))
        assertFalse(NativeNodeBridge.isLibraryLoaded)
    }
}
