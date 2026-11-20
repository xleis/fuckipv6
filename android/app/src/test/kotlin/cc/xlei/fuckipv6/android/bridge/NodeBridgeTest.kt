package cc.xlei.fuckipv6.android.bridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class NodeBridgeTest {
    @Test
    fun configRequiresAnExplicitLoopbackStyleHttpUrl() {
        assertTrue(NodeApiConfig("http://127.0.0.1:6668").isValid)
        assertTrue(NodeApiConfig("http://10.0.2.2:6668/").isValid)
        assertTrue(NodeApiConfig("https://node.internal:8443/api").isValid)

        assertEquals("bridge.error.config_missing", NodeApiConfig("").validationError())
        assertEquals("bridge.error.config_missing", NodeApiConfig("   ").validationError())
        assertEquals("bridge.error.config_invalid", NodeApiConfig("127.0.0.1:6668").validationError())
        assertEquals("bridge.error.config_invalid", NodeApiConfig("ftp://127.0.0.1").validationError())
        assertEquals("bridge.error.config_invalid", NodeApiConfig("http://").validationError())
        assertEquals("bridge.error.config_invalid", NodeApiConfig("http://127.0.0.1 :1").validationError())
        assertNull(NodeApiConfig("http://127.0.0.1:6668/").validationError())
    }

    @Test
    fun snapshotUrlKeepsTheVersionedPathExactlyOnce() {
        assertEquals(
            "http://127.0.0.1:6668/api/v1/snapshot",
            NodeApiConfig("http://127.0.0.1:6668").snapshotUrl,
        )
        assertEquals(
            "http://127.0.0.1:6668/api/v1/snapshot",
            NodeApiConfig("http://127.0.0.1:6668/").snapshotUrl,
        )
    }

    @Test
    fun failureKeysStayStableAndReuseExistingSurfaceCopy() {
        assertEquals("bridge.error.unauthorized", BridgeFailure.Unauthorized(401).key)
        assertEquals("state.permission_denied.detail", BridgeFailure.Unauthorized(401).uiStateKey)
        assertEquals("state.offline.detail", BridgeFailure.Unreachable.uiStateKey)
        assertEquals("state.offline.detail", BridgeFailure.HttpStatus(503).uiStateKey)
        assertEquals("state.error.detail", BridgeFailure.Malformed.uiStateKey)
        assertEquals("state.unsupported.detail", BridgeFailure.Misconfigured.uiStateKey)
    }

    @Test
    fun successfulFetchBecomesReadyWithProjectedFileState() {
        val view = SnapshotFileView(
            capability = FileCapabilityView(
                state = "enabled",
                networkId = "net-1",
                accessEpoch = 4,
                shareRevision = 11,
                availableBytes = 2L * 1024 * 1024 * 1024,
                activeTransfers = 0,
                rootError = null,
            ),
            networks = listOf(NetworkLabel("net-1", "家庭网络")),
            snapshotRevision = 7,
        )
        val state = NodeBridge.toBridgeState(SnapshotResult.Ok(view))
        assertTrue(state is BridgeState.Ready)
        state as BridgeState.Ready
        assertEquals(7L, state.snapshotRevision)
        assertEquals("status.capability.enabled", state.data.stateKey)
        assertEquals("家庭网络", state.data.networkLabel)
        assertEquals("2.0 GiB", state.data.formattedAvailable)
    }

    @Test
    fun failuresNeverLookLikeAReadySnapshot() {
        val unauthorized = NodeBridge.toBridgeState(
            SnapshotResult.Failed(BridgeFailure.Unauthorized(401)),
        )
        assertTrue(unauthorized is BridgeState.Failed)
        assertFalse(unauthorized.isReady)
        assertEquals(
            "bridge.error.unauthorized",
            (unauthorized as BridgeState.Failed).failure.key,
        )

        val malformed = NodeBridge.toBridgeState(SnapshotResult.Failed(BridgeFailure.Malformed))
        assertEquals("state.error.detail", (malformed as BridgeState.Failed).failure.uiStateKey)
    }

    @Test
    fun unconfiguredBridgeIsNotReportedAsAReadyOrFailedPoll() {
        val bridge = NodeBridge(source = { SnapshotResult.Failed(BridgeFailure.Unreachable) })
        assertEquals(NodeBridge.notConfigured(), NodeBridge.notConfigured())
        assertFalse(bridge.state().value is BridgeState.Ready)
    }

    @Test
    fun pollOnceReplacesStateWithTheFreshResult() {
        var next: SnapshotResult = SnapshotResult.Failed(BridgeFailure.Unreachable)
        val bridge = NodeBridge(
            source = {
                next
            },
        )
        bridge.pollOnce()
        assertTrue(bridge.state().value is BridgeState.Failed)

        next = SnapshotResult.Ok(
            SnapshotFileView(
                capability = null,
                networks = emptyList(),
                snapshotRevision = 1,
            ),
        )
        bridge.pollOnce()
        val state = bridge.state().value
        assertTrue(state is BridgeState.Ready)
        assertEquals("status.capability.disabled", (state as BridgeState.Ready).data.stateKey)
    }
}
