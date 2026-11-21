package cc.xlei.fuckipv6.android.bridge

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ExitSurfaceTest {
    private val snapshot = JSONObject(
        """
        {
          "local_exit": {
            "owner_exit_enabled": true,
            "stage": "enabled",
            "healthy": true,
            "network_allowed": true,
            "active_clients": 2,
            "total_clients": 9
          },
          "exit_selections": [
            {
              "network_id": "net-1",
              "policy_revision": 3,
              "resolution": {
                "mode": "fixed",
                "fixed_exit": "node-exit",
                "selected_default": "node-exit",
                "selectable_exits": ["node-exit", "node-other"],
                "candidates": [{"exit": "node-exit"}]
              },
              "evaluated_at_unix_ms": 500
            },
            {"policy_revision": 1}
          ]
        }
        """.trimIndent(),
    )

    @Test
    fun localExitKeepsBothConsentsSeparate() {
        val local = ExitSurface.parseLocalExit(snapshot)!!
        assertTrue(local.ownerExitEnabled)
        assertTrue(local.networkAllowed)
        assertEquals("enabled", local.stage)
        assertEquals(2, local.activeClients)
        assertEquals(9L, local.totalClients)
        assertTrue(local.serving)
    }

    @Test
    fun aNodeIsNotServingWhenEitherSideWithholdsConsent() {
        val blocked = ExitSurface.parseLocalExit(
            JSONObject(
                """{"local_exit":{"owner_exit_enabled":true,"network_allowed":false,"stage":"waiting_for_network_authorization","healthy":true}}""",
            ),
        )!!
        assertFalse(blocked.serving)
        assertEquals("waiting_for_network_authorization", blocked.stage)
    }

    @Test
    fun selectionResolutionIsProjectedWithCounts() {
        val selections = ExitSurface.parseSelections(snapshot)!!
        assertEquals(1, selections.size)
        val selection = selections.single()
        assertEquals("net-1", selection.networkId)
        assertEquals(3L, selection.policyRevision)
        assertEquals("fixed", selection.mode)
        assertEquals("node-exit", selection.fixedExit)
        assertEquals("node-exit", selection.selectedDefault)
        assertEquals(2, selection.selectableCount)
        assertEquals(1, selection.candidateCount)
        assertEquals(500L, selection.evaluatedAtUnixMs)
    }

    @Test
    fun malformedPayloadsAreUnobservedRatherThanEmpty() {
        assertNull(ExitSurface.parseLocalExit(JSONObject("{}")))
        assertNull(ExitSurface.parseSelections(JSONObject("{}")))
        assertNull(ExitSurface.parseSelections(JSONObject("""{"exit_selections":"nope"}""")))
        assertTrue(ExitSurface.parseSelections(JSONObject("""{"exit_selections":[]}"""))!!.isEmpty())
    }

    @Test
    fun localExitCommandCarriesOnlyTheOwnerFlag() {
        val enable = ExitSurface.localExitCommand(true)
        assertEquals("set_local_exit_capability", enable.getString("command"))
        assertTrue(enable.getJSONObject("payload").getBoolean("enabled"))
        val disable = ExitSurface.localExitCommand(false)
        assertFalse(disable.getJSONObject("payload").getBoolean("enabled"))
    }

    @Test
    fun theCommandProjectionParsesWithoutAnEnvelope() {
        val projection = ExitSurface.parseLocalExitObject(
            JSONObject(
                """{"owner_exit_enabled":false,"network_allowed":true,"stage":"disabled","healthy":false,"active_clients":0,"total_clients":3}""",
            ),
        )
        assertFalse(projection.ownerExitEnabled)
        assertTrue(projection.networkAllowed)
        assertEquals("disabled", projection.stage)
        assertFalse(projection.serving)
        assertEquals(3L, projection.totalClients)
    }
}
