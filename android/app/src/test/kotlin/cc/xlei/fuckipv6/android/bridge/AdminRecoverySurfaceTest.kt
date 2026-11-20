package cc.xlei.fuckipv6.android.bridge

import cc.xlei.fuckipv6.android.ui.AdminRecoveryPageData
import cc.xlei.fuckipv6.android.ui.AdminRecoveryRequestCodec
import cc.xlei.fuckipv6.android.ui.AdminRecoveryStatusProjection
import cc.xlei.fuckipv6.contract.NodeContractV1.AdminRecoveryState
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AdminRecoverySurfaceTest {
    private val listPayload = JSONObject(
        """
        {"items":[
          {
            "request_id":"req-1","network_id":"net-1","state":"pending",
            "can_execute":true,"denial_key":null,"initiator":"node-initiator",
            "participants":["node-a","node-b"],"signature_count":2,"required_signatures":2,
            "issued_at_unix_ms":1760000000000,"expires_at_unix_ms":1760000600000,
            "impact":{"administrators_added":[],"administrators_removed":["node-old"],
                      "members_added":["node-new"],"members_removed":[],
                      "owner_after":"node-owner-b","owner_before":"node-owner-a"},
            "revocation":{"old_sessions":true,"pending_invitations":true,"unconfirmed_writes":false}
          },
          {"request_id":"req-2","network_id":"net-1","state":"something_new","can_execute":false}
        ]}
        """.trimIndent(),
    )

    @Test
    fun listCommandUsesTheDocumentedCommandName() {
        val command = AdminRecoverySurface.listCommand("家庭网络")
        assertEquals("list_admin_recoveries", command.getString("command"))
        assertEquals("家庭网络", command.getJSONObject("payload").getString("network_name"))
    }

    @Test
    fun parsesRecoveriesIntoTypedProjections() {
        val views = AdminRecoverySurface.parseList(listPayload)
        assertEquals(2, views.size)
        val projection = AdminRecoveryStatusProjection.from(views[0])
        assertEquals("req-1", projection.requestId)
        assertEquals(AdminRecoveryState.PENDING, projection.state)
        assertTrue(projection.canExecute!!)
        assertEquals(listOf("node-a", "node-b"), projection.participants)
        assertEquals(2L, projection.requiredSignatures)
        assertEquals("node-owner-b", projection.impact?.ownerAfter)
        assertEquals(listOf("node-old"), projection.impact?.administratorsRemoved)
        assertTrue(projection.revocation?.oldSessions == true)
        assertFalse(projection.revocation?.unconfirmedWrites!!)
        assertNull(projection.denialKey)
    }

    @Test
    fun unknownStateIsNullRatherThanAGuess() {
        val views = AdminRecoverySurface.parseList(listPayload)
        val projection = AdminRecoveryStatusProjection.from(views[1])
        assertNull(projection.state)
        assertEquals("req-2", projection.requestId)
        assertFalse(projection.canExecute!!)
        assertNull(projection.impact)
        assertNull(projection.revocation)
    }

    @Test
    fun missingPayloadMeansNoRecoveriesNotACrash() {
        assertTrue(AdminRecoverySurface.parseList(null).isEmpty())
        assertTrue(AdminRecoverySurface.parseList(JSONObject("{}")).isEmpty())
        val skipped = AdminRecoverySurface.parseList(JSONObject("""{"items":[1,"x",{"request_id":"req-9"}]}"""))
        assertEquals(1, skipped.size)
        assertEquals("req-9", skipped[0].requestId)
        assertNull(skipped[0].state)
    }

    @Test
    fun bridgePageWiresOnlyTheRefreshAction() {
        val views = AdminRecoverySurface.parseList(listPayload)
        val page = AdminRecoveryPageData.fromBridge(
            networkName = "家庭网络",
            networkId = "net-1",
            recoveries = views,
            onList = {},
        )
        assertEquals("net-1", page.networkId)
        assertEquals(2, page.recoveries.size)
        assertEquals("req-1", page.latestStatus?.requestId)
        assertTrue(page.backendAvailable)
        assertNull(page.onSubmit)
        assertNull(page.onExecute)
        assertNull(page.onRevoke)

        val withoutChannel = AdminRecoveryPageData.fromBridge("家庭网络", "net-1", views)
        assertFalse(withoutChannel.backendAvailable)
    }
    @Test
    fun submittedBundleRoundTripsThroughTheDocumentedKeys() {
        val requestId = "recovery_" + java.util.Base64.getUrlEncoder().withoutPadding()
            .encodeToString(ByteArray(32) { index -> index.toByte() })
        val raw = """
        {
          "proposal": {
            "schema_version": 1,
            "request_id": "$requestId",
            "network_id": "net-1",
            "snapshot": {
              "owner": "node-owner-a",
              "administrators": ["node-a"],
              "members": ["node-a", "node-b"],
              "network_revision": 7,
              "roster_digest": "digest-7"
            },
            "target": {
              "owner": "node-owner-b",
              "administrators": [],
              "members": ["node-b"]
            },
            "initiated_by": "node-owner-a",
            "reason": "disaster_recovery",
            "nonce": "nonce-1",
            "issued_at_unix_ms": 1760000000000,
            "expires_at_unix_ms": 1760000600000,
            "revocation": {
              "old_sessions": true,
              "pending_invitations": true,
              "unconfirmed_writes": false
            }
          },
          "approvals": [
            {
              "signer": "trustee-1",
              "signer_public_key": "cHVibGljLWtleS0x",
              "signed_at_unix_ms": 1760000001000,
              "signature": "c2lnbmF0dXJlLTE"
            }
          ]
        }
        """.trimIndent()
        val parsed = AdminRecoveryRequestCodec.parse(raw)
        val encoded = AdminRecoverySurface.encodeRequest(parsed)
        val reparsed = AdminRecoveryRequestCodec.parse(encoded.toString())
        assertEquals(parsed.proposal.request_id, reparsed.proposal.request_id)
        assertEquals(parsed.proposal.nonce, reparsed.proposal.nonce)
        assertEquals(parsed.proposal.target.owner, reparsed.proposal.target.owner)
        assertEquals(
            parsed.proposal.snapshot.network_revision,
            reparsed.proposal.snapshot.network_revision,
        )
        assertEquals(parsed.approvals.size, reparsed.approvals.size)
        assertEquals(parsed.approvals[0].signature, reparsed.approvals[0].signature)
        assertEquals(
            parsed.proposal.revocation.unconfirmed_writes,
            reparsed.proposal.revocation.unconfirmed_writes,
        )

        val command = AdminRecoverySurface.submitCommand("家庭网络", parsed)
        assertEquals("submit_admin_recovery", command.getString("command"))
        assertEquals("家庭网络", command.getJSONObject("payload").getString("network_name"))
        assertTrue(command.getJSONObject("payload").getJSONObject("request").has("proposal"))
        assertTrue(command.getJSONObject("payload").getJSONObject("request").has("approvals"))
    }

    @Test
    fun statusRevokeAndExecuteCommandsCarryOnlyDocumentedFields() {
        val status = AdminRecoverySurface.statusCommand("n", "req-1")
        assertEquals("get_admin_recovery_status", status.getString("command"))
        assertEquals(
            setOf("network_name", "request_id"),
            status.getJSONObject("payload").keys().asSequence().toSet(),
        )

        val revoke = AdminRecoverySurface.revokeCommand("n", "req-1", "operator request")
        assertEquals("revoke_admin_recovery", revoke.getString("command"))
        assertEquals("operator request", revoke.getJSONObject("payload").getString("reason"))

        val execute = AdminRecoverySurface.executeCommand("n", "req-1")
        assertEquals("execute_admin_recovery", execute.getString("command"))
        assertEquals(
            setOf("network_name", "request_id"),
            execute.getJSONObject("payload").keys().asSequence().toSet(),
        )
    }
}
