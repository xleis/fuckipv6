package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class MemberSurfaceTest {
    private val directory = JSONObject(
        """
        {
          "network_id": "net-1",
          "viewer_role": "administrator",
          "can_manage_members": true,
          "network_revision": 12,
          "members": [
            {
              "node_id": "node-a",
              "display_name": "小明",
              "role": "owner",
              "status": "online",
              "lifecycle_state": "active",
              "revision": 3,
              "access_epoch": 5,
              "available_actions": ["demote", "remove"],
              "joined_at_unix_ms": 1760000000000
            },
            {"node_id": "node-b", "role": "member", "available_actions": []},
            {"role": "member"},
            "not-an-object"
          ]
        }
        """.trimIndent(),
    )

    @Test
    fun listCommandUsesTheDocumentedCommandAndPayload() {
        val command = MemberSurface.listCommand("net-1")
        assertEquals("list_network_members", command.getString("command"))
        assertEquals("net-1", command.getJSONObject("payload").getString("network_id"))
    }

    @Test
    fun directoryKeepsOnlyPublishingMembersAndTheirAuthorizedActions() {
        val parsed = MemberSurface.parseDirectory(directory)!!
        assertEquals("net-1", parsed.networkId)
        assertEquals("administrator", parsed.viewerRole)
        assertTrue(parsed.canManageMembers)
        assertEquals(12L, parsed.networkRevision)
        assertEquals(2, parsed.members.size)

        val first = parsed.members[0]
        assertEquals("node-a", first.nodeId)
        assertEquals("小明", first.displayName)
        assertEquals("owner", first.role)
        assertEquals("online", first.status)
        assertEquals("active", first.lifecycleState)
        assertEquals(3L, first.revision)
        assertEquals(5L, first.accessEpoch)
        assertEquals(listOf("demote", "remove"), first.availableActions)
        assertEquals(1760000000000L, first.joinedAtUnixMs)

        val second = parsed.members[1]
        assertNull(second.displayName)
        assertTrue(second.availableActions.isEmpty())
    }

    @Test
    fun emptyRosterIsValidButNonDirectoryPayloadIsNot() {
        val empty = MemberSurface.parseDirectory(
            JSONObject("""{"network_id":"net-1","members":[]}"""),
        )!!
        assertTrue(empty.members.isEmpty())
        assertFalse(empty.canManageMembers)

        val missingMembers = MemberSurface.parseDirectory(JSONObject("""{"network_id":"net-1"}"""))!!
        assertTrue(missingMembers.members.isEmpty())

        assertNull(MemberSurface.parseDirectory(null))
        assertNull(MemberSurface.parseDirectory(JSONObject("""{"members":[]}""")))
        assertNull(
            MemberSurface.parseDirectory(
                JSONObject("""{"network_id":"net-1","members":"nope"}"""),
            ),
        )
    }

    @Test
    fun unknownRolesAndStatesArePreservedVerbatim() {
        val parsed = MemberSurface.parseDirectory(
            JSONObject(
                """{"network_id":"net-1","members":[{"node_id":"node-c","role":"future_role","lifecycle_state":"future_state"}]}""",
            ),
        )!!
        assertEquals("future_role", parsed.members[0].role)
        assertEquals("future_state", parsed.members[0].lifecycleState)
    }
}
