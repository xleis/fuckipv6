package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BlacklistSurfaceTest {
    @Test
    fun commandsCarryTheDocumentedNamesAndTrimmedInput() {
        val status = BlacklistSurface.statusCommand(" 家庭网络 ")!!
        assertEquals("get_blacklist", status.getString("command"))
        val statusPayload = status.getJSONObject("payload")
        assertEquals("家庭网络", statusPayload.getString("network_name"))
        assertFalse(statusPayload.has("client_id"))

        val ban = BlacklistSurface.banCommand("家庭网络", " client-1 ")!!
        assertEquals("ban_client", ban.getString("command"))
        assertEquals("client-1", ban.getJSONObject("payload").getString("client_id"))

        val unban = BlacklistSurface.unbanCommand("家庭网络", "client-1")!!
        assertEquals("unban_client", unban.getString("command"))
    }

    @Test
    fun blankIdentifiersNeverReachTheWire() {
        assertNull(BlacklistSurface.statusCommand(" "))
        assertNull(BlacklistSurface.banCommand("家庭网络", "   "))
        assertNull(BlacklistSurface.unbanCommand("", "client-1"))
    }

    @Test
    fun snapshotParsesClientsAndChangeFlag() {
        val view = BlacklistSurface.parse(
            JSONObject("""{"network_name":"家庭网络","clients":["a","b"],"changed":true}"""),
        )!!
        assertEquals("家庭网络", view.networkName)
        assertEquals(listOf("a", "b"), view.clients)
        assertTrue(view.changed)

        val idempotent = BlacklistSurface.parse(
            JSONObject("""{"network_name":"家庭网络","clients":["a"],"changed":false}"""),
        )!!
        assertFalse(idempotent.changed)
    }

    @Test
    fun missingOrMalformedSnapshotsStayUnobserved() {
        assertNull(BlacklistSurface.parse(null))
        assertNull(BlacklistSurface.parse(JSONObject("{}")))
        assertNull(BlacklistSurface.parse(JSONObject("""{"network_name":"n","clients":"x"}""")))
        val empty = BlacklistSurface.parse(JSONObject("""{"network_name":"n"}"""))!!
        assertTrue(empty.clients.isEmpty())
        assertFalse(empty.changed)
    }
}
