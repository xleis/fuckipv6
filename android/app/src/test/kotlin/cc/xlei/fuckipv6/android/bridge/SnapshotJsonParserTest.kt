package cc.xlei.fuckipv6.android.bridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SnapshotJsonParserTest {
    private fun envelope(snapshot: String) =
        """{"schema_version":1,"request_id":"r1","server_time":"2026-11-20T10:00:00Z","snapshot":$snapshot}"""

    @Test
    fun readsCapabilityMembershipsAndRevision() {
        val body = envelope(
            """
            {
              "revision": 42,
              "file_capability": {
                "state": "enabled",
                "network_id": "net-1",
                "access_epoch": 3,
                "share_revision": 9,
                "available_bytes": 5368709120,
                "active_transfers": 1,
                "root_error": null
              },
              "memberships": [
                {"network_id": "net-1", "network_name": "家庭网络"},
                {"network_id": "net-2"}
              ]
            }
            """.trimIndent(),
        )
        val view = SnapshotJsonParser.parse(body).getOrThrow()
        assertEquals(42L, view.snapshotRevision)
        val capability = assertNotNull(view.capability).let { view.capability!! }
        assertEquals("enabled", capability.state)
        assertEquals("net-1", capability.networkId)
        assertEquals(3L, capability.accessEpoch)
        assertEquals(9L, capability.shareRevision)
        assertEquals(5_368_709_120L, capability.availableBytes)
        assertEquals(1L, capability.activeTransfers)
        assertNull(capability.rootError)
        // A membership without a network name cannot be used as a label.
        assertEquals(listOf(NetworkLabel("net-1", "家庭网络")), view.networks)
    }

    @Test
    fun missingCapabilityStaysMissing() {
        val view = SnapshotJsonParser.parse(envelope("""{"revision": 1}""")).getOrThrow()
        assertNull(view.capability)
        assertTrue(view.networks.isEmpty())
        // The projection turns a missing capability into `disabled`, never into
        // an invented "enabled" state.
        assertEquals("status.capability.disabled", view.toPageData().stateKey)
    }

    @Test
    fun unknownStateStringsArePreservedForFailClosedProjection() {
        val view = SnapshotJsonParser.parse(
            envelope("""{"file_capability": {"state": "SOMETHING_NEW"}}"""),
        ).getOrThrow()
        assertEquals("SOMETHING_NEW", view.capability?.state)
        assertEquals("status.capability.unknown", view.toPageData().stateKey)
    }

    @Test
    fun numericStringsAreAccepted() {
        val view = SnapshotJsonParser.parse(
            envelope(
                """{"file_capability": {"state": "enabled", "available_bytes": "1048576", "active_transfers": "2"}}""",
            ),
        ).getOrThrow()
        assertEquals(1_048_576L, view.capability?.availableBytes)
        assertEquals(2L, view.capability?.activeTransfers)
    }

    @Test
    fun malformedOrUnknownEnvelopesFailClosed() {
        assertTrue(SnapshotJsonParser.parse("not json").isFailure)
        assertTrue(SnapshotJsonParser.parse("""{"request_id":"r1"}""").isFailure)
        assertTrue(SnapshotJsonParser.parse("""[]""").isFailure)
    }

    @Test
    fun rootErrorIsCarriedAndLaterReducedToAStableCode() {
        val view = SnapshotJsonParser.parse(
            envelope("""{"file_capability": {"state":"unavailable","root_error":"file.error.root_missing"}}"""),
        ).getOrThrow()
        assertEquals("file.error.root_missing", view.capability?.rootError)
        val page = view.toPageData()
        assertEquals("file.error.root_missing", page.rootErrorCode)
        assertFalse(page.showsTransferActions)
    }
}
