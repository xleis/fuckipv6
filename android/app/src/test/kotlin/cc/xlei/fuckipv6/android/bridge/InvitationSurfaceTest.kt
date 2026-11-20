package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class InvitationSurfaceTest {
    @Test
    fun createCommandCarriesDomainBoundedValues() {
        val command = InvitationSurface.createCommand(
            CreateInvitationRequest(
                networkId = "net-1",
                role = "member",
                capabilities = listOf("file"),
                maxUses = 3,
                expiresInMs = 600_000,
            ),
        )!!
        assertEquals("create_invitation", command.getString("command"))
        val payload = command.getJSONObject("payload")
        assertEquals("net-1", payload.getString("network_id"))
        assertEquals("member", payload.getString("role"))
        assertEquals(3, payload.getInt("max_uses"))
        assertEquals(600_000L, payload.getLong("expires_in_ms"))
        assertEquals("file", payload.getJSONArray("capabilities").getString(0))
    }

    @Test
    fun outOfRangeOrUnknownValuesNeverReachTheWire() {
        assertNull(InvitationSurface.createCommand(CreateInvitationRequest("net-1", role = "superuser")))
        assertNull(InvitationSurface.createCommand(CreateInvitationRequest("net-1", maxUses = 0)))
        assertNull(InvitationSurface.createCommand(CreateInvitationRequest("net-1", maxUses = -3)))
        assertNull(
            InvitationSurface.createCommand(
                CreateInvitationRequest("net-1", expiresInMs = 1_000),
            ),
        )
        assertNull(
            InvitationSurface.createCommand(
                CreateInvitationRequest("net-1", expiresInMs = CreateInvitationRequest.MAX_EXPIRY_MS + 1),
            ),
        )
        assertNull(InvitationSurface.createCommand(CreateInvitationRequest("   ")))
        assertNull(InvitationSurface.listCommand(""))
        assertNull(InvitationSurface.revokeCommand("net-1", "inv-1", expectedRevision = 0))
    }

    @Test
    fun listParsesStatusRecordsAndSkipsBrokenEntries() {
        val list = InvitationSurface.parseList(
            JSONObject(
                """
                {"items":[
                  {"invitation":{"invitation_id":"inv-1","role":"member","max_uses":2,
                    "used_count":1,"revision":4,"created_at_unix_ms":10,"expires_at_unix_ms":20},
                   "state":"active","remaining_uses":1},
                  {"invitation":{"role":"member"},"state":"active"},
                  "not-an-object"
                ]}
                """.trimIndent(),
            ),
        )
        assertEquals(1, list.size)
        val first = list.single()
        assertEquals("inv-1", first.invitationId)
        assertEquals("active", first.state)
        assertEquals(2L, first.maxUses)
        assertEquals(1L, first.remainingUses)
        assertEquals(4L, first.revision)
        assertNull(first.revokedAtUnixMs)

        assertTrue(InvitationSurface.parseList(null).isEmpty())
        assertTrue(InvitationSurface.parseList(JSONObject("{}")).isEmpty())
    }

    @Test
    fun issuedInvitationKeepsTheSingleReadCodeAndItsStatus() {
        val issued = InvitationSurface.parseIssued(
            JSONObject(
                """{"code":"INVITE-CODE-1","status":{"invitation":{"invitation_id":"inv-2","role":"member","revision":1},"state":"active","remaining_uses":1}}""",
            ),
        )!!
        assertEquals("INVITE-CODE-1", issued.code)
        assertEquals("inv-2", issued.invitation?.invitationId)
        assertEquals(1L, issued.invitation?.remainingUses)

        assertNull(InvitationSurface.parseIssued(null))
        assertNull(InvitationSurface.parseIssued(JSONObject("""{"status":{}}""")))
        assertFalse(issued.code.isEmpty())
    }
}
