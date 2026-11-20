package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LogsIdentitySurfaceTest {
    private val snapshot = JSONObject(
        """
        {
          "identity": {
            "node_id": "node-a",
            "display_name": "家里的节点",
            "public_key_fingerprint": "SHA256:abc",
            "created_at_unix_ms": 100
          },
          "logs": [
            {"unix_ms": 10, "level": "info", "target": "node", "message": "started"},
            {"unix_ms": 20, "level": "error", "target": "tun", "message": "device missing"},
            "not-an-object"
          ]
        }
        """.trimIndent(),
    )

    @Test
    fun publishedLogLinesKeepTheirOrder() {
        val feed = LogsSurface.parse(snapshot)!!
        assertTrue(feed.hasLines)
        assertEquals(2, feed.lines.size)
        assertEquals("info", feed.lines[0].level)
        assertEquals("device missing", feed.lines[1].message)
        assertEquals(20L, feed.lines[1].unixMs)
    }

    @Test
    fun anEmptyLogArrayIsNotFilledWithFabricatedLines() {
        val feed = LogsSurface.parse(JSONObject("{\"logs\":[]}"))!!
        assertFalse(feed.hasLines)
        assertTrue(feed.lines.isEmpty())
    }

    @Test
    fun messagesAreBoundedAndMalformedPayloadsAreUnobserved() {
        val long = "x".repeat(LogsSurface.MAX_MESSAGE_CHARS + 50)
        val feed = LogsSurface.parse(
            JSONObject("""{"logs":[{"level":"info","target":"t","message":"$long"}]}"""),
        )!!
        assertEquals(LogsSurface.MAX_MESSAGE_CHARS, feed.lines.single().message.length)

        assertNull(LogsSurface.parse(JSONObject("{}")))
        assertNull(LogsSurface.parse(JSONObject("""{"logs":"nope"}""")))
    }

    @Test
    fun identityDistinguishesHiddenFieldsFromMissingOnes() {
        val identity = IdentitySurface.parse(snapshot)!!
        assertEquals("node-a", identity.nodeId)
        assertEquals("家里的节点", identity.displayName)
        assertEquals("SHA256:abc", identity.fingerprint)
        assertFalse(identity.displayNameHidden)

        val hidden = IdentitySurface.parse(
            JSONObject("""{"identity":{"node_id":"node-b"}}"""),
        )!!
        assertTrue(hidden.displayNameHidden)
        assertTrue(hidden.fingerprintHidden)
        assertNull(hidden.createdAtUnixMs)

        assertNull(IdentitySurface.parse(JSONObject("{}")))
        assertNull(IdentitySurface.parse(JSONObject("""{"identity":{"display_name":"x"}}""")))
    }

    @Test
    fun logOrderIsBoundedFromTheTail() {
        val lines = (0 until (LogsSurface.MAX_LINES + 10)).joinToString(",") { index ->
            """{"level":"info","target":"t","message":"line-$index"}"""
        }
        val feed = LogsSurface.parse(JSONObject("""{"logs":[$lines]}"""))!!
        assertEquals(LogsSurface.MAX_LINES, feed.lines.size)
        assertEquals("line-${LogsSurface.MAX_LINES + 9}", feed.lines.last().message)
    }
}
