package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MemberLifecycleTest {
    private fun request(
        action: String = "suspend",
        role: String = "member",
        revision: Long = 4,
        reason: String? = null,
    ) = MemberLifecycleRequest(
        networkId = "net-1",
        targetNodeId = "node-a",
        action = action,
        role = role,
        expectedRevision = revision,
        reason = reason,
    )

    @Test
    fun commandCarriesTheRevisionGuardAndNothingElse() {
        val command = MemberLifecycleSurface.command(request())!!
        assertEquals("change_member_lifecycle", command.getString("command"))
        val payload = command.getJSONObject("payload")
        assertEquals("net-1", payload.getString("network_id"))
        assertEquals("node-a", payload.getString("target_node_id"))
        assertEquals("suspend", payload.getString("action"))
        assertEquals("member", payload.getString("role"))
        assertEquals(4L, payload.getLong("expected_revision"))
        assertFalse(payload.has("reason"))

        val withReason = MemberLifecycleSurface.command(request(reason = " 政策违规 "))!!
        assertEquals(" 政策违规 ", withReason.getJSONObject("payload").getString("reason"))
    }

    @Test
    fun unknownActionsRolesAndStaleInputsAreRejectedLocally() {
        assertNull(MemberLifecycleSurface.command(request(action = "delete_everything")))
        assertNull(MemberLifecycleSurface.command(request(role = "superuser")))
        assertNull(MemberLifecycleSurface.command(request(revision = -1)))
        assertNull(
            MemberLifecycleSurface.command(request().copy(networkId = "  ")),
        )
        assertNull(
            MemberLifecycleSurface.command(request().copy(targetNodeId = "")),
        )

        for (action in MemberLifecycleSurface.ACTIONS) {
            assertTrue(
                "action $action must encode",
                MemberLifecycleSurface.command(request(action = action)) != null,
            )
        }
    }

    @Test
    fun changeResultKeepsTheAuthoritativeAfterEffects() {
        val change = MemberLifecycleSurface.parseChange(
            JSONObject(
                """
                {"network_id":"net-1","target_node_id":"node-a","action":"suspend",
                 "status":"suspended","revision":5,"access_epoch":9,"operation_id":"op-1",
                 "idempotent":false,"access_revoked":true,"requires_reauthentication":true,
                 "requires_reapproval":false}
                """.trimIndent(),
            ),
        )!!
        assertEquals("suspended", change.status)
        assertEquals(5L, change.revision)
        assertEquals(9L, change.accessEpoch)
        assertTrue(change.accessRevoked)
        assertTrue(change.requiresReauthentication)
        assertFalse(change.requiresReapproval)
    }

    @Test
    fun missingPayloadStaysUnobserved() {
        assertNull(MemberLifecycleSurface.parseChange(null))
        val sparse = MemberLifecycleSurface.parseChange(JSONObject("{}"))!!
        assertNull(sparse.status)
        assertNull(sparse.revision)
        assertFalse(sparse.idempotent)
    }
}
