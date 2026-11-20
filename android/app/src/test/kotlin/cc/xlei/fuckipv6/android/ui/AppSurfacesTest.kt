package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.bridge.BridgeState
import cc.xlei.fuckipv6.android.bridge.CoordinatorCandidateView
import cc.xlei.fuckipv6.android.bridge.CoordinatorSurface
import cc.xlei.fuckipv6.android.bridge.CoordinatorTermView
import cc.xlei.fuckipv6.android.bridge.FileCapabilityView
import cc.xlei.fuckipv6.android.bridge.NetworkLabel
import cc.xlei.fuckipv6.android.bridge.RecoveryStatusView
import cc.xlei.fuckipv6.android.bridge.RelaySurface
import cc.xlei.fuckipv6.android.bridge.SnapshotFileView
import cc.xlei.fuckipv6.contract.NodeContractV1.NodeExecuteAdminRecoveryCommand
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class AppSurfacesTest {
    private class RecordingHandlers : SurfaceHandlers {
        val calls = mutableListOf<String>()
        override fun setRelayRole(enabled: Boolean) {
            calls += "relay:$enabled"
        }
        override fun submitRecovery(command: cc.xlei.fuckipv6.contract.NodeContractV1.NodeSubmitAdminRecoveryCommand) {
            calls += "submit"
        }
        override fun getRecoveryStatus(command: cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryStatusCommand) {
            calls += "status"
        }
        override fun listRecoveries(command: cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryListCommand) {
            calls += "list"
        }
        override fun revokeRecovery(command: cc.xlei.fuckipv6.contract.NodeContractV1.NodeRevokeAdminRecoveryCommand) {
            calls += "revoke"
        }
        override fun executeRecovery(command: NodeExecuteAdminRecoveryCommand) {
            calls += "execute"
        }
        override fun saveNodeApi(url: String, token: String): SettingsValidation {
            calls += "save:$url"
            return SettingsValidation.of(url, token)
        }
        override fun clearNodeApi() {
            calls += "clear"
        }
    }

    private val handlers = RecordingHandlers()
    private val ready = BridgeState.Ready(
        data = FilePageData.from(
            FileCapabilityView("enabled", "net-1", 1, 2, 1024, 0, null),
            listOf(NetworkLabel("net-1", "家庭网络")),
        ),
        snapshotRevision = 9,
        relay = RelaySurface(
            networkName = "家庭网络",
            relayEnabled = true,
            health = null,
            governance = null,
            permissions = emptySet(),
        ),
        coordinator = CoordinatorSurface(
            networkName = "家庭网络",
            controlPlaneState = "available",
            governanceWritable = true,
            reasonCode = null,
            receivedVotes = 2,
            requiredVotes = 2,
            participantCount = 3,
            currentLeader = "node-a",
            latestTerm = CoordinatorTermView(3, 3, "node-a", "stable"),
            candidates = listOf(
                CoordinatorCandidateView("node-a", 1, "healthy", true, "reason"),
            ),
        ),
        networks = listOf(NetworkLabel("net-1", "家庭网络")),
    )

    private fun resolve(state: BridgeState, recoveries: List<RecoveryStatusView> = emptyList()) =
        SurfaceResolver.resolve(
            bridgeState = state,
            recoveries = recoveries,
            browse = FileWorkspaceState(),
            transfers = FileTransferState(),
            settingsUrl = "http://127.0.0.1:6668",
            settingsHasToken = true,
            configured = true,
            handlers = handlers,
        )

    @Test
    fun readySnapshotFillsEverySurfaceFromTheAuthoritativeState() {
        val surfaces = resolve(ready)
        assertNotNull(surfaces.files)
        assertEquals("status.capability.enabled", surfaces.files!!.stateKey)
        assertEquals(9L, surfaces.filesRevision)
        assertNotNull(surfaces.relay)
        assertEquals("家庭网络", surfaces.relay!!.networkName)
        assertTrue(surfaces.relay!!.backendAvailable)
        assertNotNull(surfaces.coordinator)
        assertEquals("runtime.control_plane.available", surfaces.coordinator!!.stateKey)
        assertNotNull(surfaces.adminRecovery)
        assertEquals("net-1", surfaces.adminRecovery!!.networkId)
        assertTrue(surfaces.adminRecovery!!.backendAvailable)
        assertNotNull(surfaces.settings)
        assertTrue(surfaces.settings!!.connected)
        assertTrue(surfaces.anySurfaceReady)
    }

    @Test
    fun unconfiguredBridgeStillExposesSettingsAndNoDataSurfaces() {
        val surfaces = resolve(BridgeState.NotConfigured)
        assertNull(surfaces.files)
        assertNull(surfaces.relay)
        assertNull(surfaces.coordinator)
        assertNull(surfaces.adminRecovery)
        assertNull(surfaces.filesFailureKey)
        assertNotNull(surfaces.settings)
        assertFalse(surfaces.anySurfaceReady)
    }

    @Test
    fun failedBridgeReportsAReasonAndNeverKeepsStaleSurfaces() {
        val surfaces = resolve(BridgeState.Failed(BridgeFailure.Unauthorized(401)))
        assertNull(surfaces.files)
        assertNull(surfaces.relay)
        assertEquals("state.permission_denied.detail", surfaces.filesFailureKey)
        assertFalse(surfaces.anySurfaceReady)
    }

    @Test
    fun surfaceCallbacksReachTheHandlers() {
        val surfaces = resolve(ready)
        surfaces.relay!!.onSetRelayRole?.invoke(true)
        surfaces.adminRecovery!!.onList?.invoke(
            cc.xlei.fuckipv6.contract.NodeContractV1.NodeAdminRecoveryListCommand("家庭网络"),
        )
        surfaces.settings!!.onClear()
        assertEquals(listOf("relay:true", "list", "clear"), handlers.calls)
    }

    @Test
    fun recoveriesAreProjectedIntoTheAdminSurface() {
        val surfaces = resolve(
            ready,
            recoveries = listOf(
                RecoveryStatusView(
                    requestId = "req-1",
                    networkId = "net-1",
                    state = "pending",
                    canExecute = true,
                    denialKey = null,
                    initiator = "node-a",
                    participants = listOf("node-a"),
                    signatureCount = 2,
                    requiredSignatures = 2,
                    issuedAtUnixMs = 1,
                    expiresAtUnixMs = 2,
                    impact = null,
                    revocation = null,
                ),
            ),
        )
        assertEquals(1, surfaces.adminRecovery!!.recoveries.size)
        assertEquals("req-1", surfaces.adminRecovery!!.latestStatus?.requestId)
        assertTrue(surfaces.adminRecovery!!.latestStatus!!.canExecute == true)
    }

    @Test
    fun adminSurfaceIsAbsentWhenNoNetworkIsKnown() {
        val lonely = BridgeState.Ready(
            data = FilePageData.from(null),
            snapshotRevision = null,
        )
        val surfaces = resolve(lonely)
        assertNull(surfaces.adminRecovery)
        assertNull(surfaces.relay)
        assertNull(surfaces.coordinator)
    }
}
