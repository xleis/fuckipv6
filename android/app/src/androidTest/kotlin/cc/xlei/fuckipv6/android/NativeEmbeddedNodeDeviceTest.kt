package cc.xlei.fuckipv6.android

import android.os.Build
import android.util.Log
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import cc.xlei.fuckipv6.android.bridge.CommandJson
import cc.xlei.fuckipv6.android.bridge.HttpSnapshotSource
import cc.xlei.fuckipv6.android.bridge.NetworkWorkflowSurface
import cc.xlei.fuckipv6.android.bridge.NodeApiConfig
import cc.xlei.fuckipv6.android.bridge.NodeCommandClient
import cc.xlei.fuckipv6.android.bridge.SnapshotResult
import cc.xlei.fuckipv6.android.bridge.TrafficPolicySurface
import cc.xlei.fuckipv6.android.service.NativeNodeBridge
import java.io.File
import java.util.UUID
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Real-device JNI integration test executing the embedded Rust node
 * (`libfuckipv6_android.so`) natively on ARM64 Android.
 */
@RunWith(AndroidJUnit4::class)
class NativeEmbeddedNodeDeviceTest {
    @Test
    fun embeddedRustNodeStartsServesLoopbackApiExecutesCommandsAndSurvivesStopNode() {
        assertTrue("libfuckipv6_android.so must load on real arm64 device", NativeNodeBridge.isLibraryLoaded)

        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val storageDir = File(context.cacheDir, "native-node-test-${UUID.randomUUID()}")
        storageDir.mkdirs()

        val callback = object : NativeNodeBridge.HostVpnCallback {
            override fun openNetwork(settingsJson: String): Int = -1
            override fun closeNetwork(networkName: String) = Unit
        }

        val started = NativeNodeBridge.startRuntime(storageDir.absolutePath, callback)
        assertTrue("startRuntime failed: ${started.exceptionOrNull()}", started.isSuccess)
        val runtime = started.getOrThrow()
        try {
            assertTrue(runtime.apiUrl.startsWith("http://127.0.0.1:"))
            assertTrue(runtime.nodeId.isNotBlank())

            val config = NodeApiConfig(baseUrl = runtime.apiUrl, token = runtime.token)
            val snapshotSource = HttpSnapshotSource(config)
            val snap = snapshotSource.fetchSnapshot()
            assertTrue("GET /api/v1/snapshot failed: $snap", snap is SnapshotResult.Ok)
            val okSnap = snap as SnapshotResult.Ok
            assertEquals(
                runtime.nodeId,
                okSnap.view.peers?.optJSONObject("identity")?.optString("node_id"),
            )

            // Unauthenticated request must fail with 401.
            val unauthSnap = HttpSnapshotSource(NodeApiConfig(baseUrl = runtime.apiUrl, token = null)).fetchSnapshot()
            assertTrue("Unauthenticated request must fail closed", unauthSnap is SnapshotResult.Failed)

            // Execute authoritative traffic policy read & stop_node over loopback HTTP API.
            val cmdClient = NodeCommandClient(config)
            val getPolicy = cmdClient.submit(
                CommandJson.envelope(TrafficPolicySurface.getPolicyCommand(), 101L, "it-get-tp"),
            )
            assertTrue(
                "get_traffic_policy must succeed: $getPolicy",
                getPolicy is cc.xlei.fuckipv6.android.bridge.CommandOutcome.Succeeded,
            )

            // W16-02 #4: stop_node (202 Accepted async op) stops data plane without terminating control HTTP API!
            val stopNode = cmdClient.submit(
                CommandJson.envelope(NetworkWorkflowSurface.stopNodeCommand(), 102L, "it-stop-node"),
            )
            assertTrue(
                "stop_node must be accepted or succeed: $stopNode",
                stopNode is cc.xlei.fuckipv6.android.bridge.CommandOutcome.Succeeded ||
                    stopNode is cc.xlei.fuckipv6.android.bridge.CommandOutcome.InProgress,
            )
            val snapAfterStop = snapshotSource.fetchSnapshot()
            assertTrue(
                "Control HTTP API must stay reachable after stop_node: $snapAfterStop",
                snapAfterStop is SnapshotResult.Ok,
            )

            assertTrue(
                "notifyNetworkPathChanged must succeed on live runtime",
                NativeNodeBridge.notifyNetworkPathChanged(runtime.handle),
            )
            val telem = NativeNodeBridge.getTelemetry(runtime.handle)
            assertNotNull("native telemetry must be available", telem)
            Log.i(
                "W16_NATIVE_NODE_TEST",
                "model=${Build.MODEL} sdk=${Build.VERSION.SDK_INT} abi=${Build.SUPPORTED_ABIS.firstOrNull()} " +
                    "node_id=${runtime.nodeId} api=${runtime.apiUrl} uptime=${telem!!.uptimeSecs}s",
            )
        } finally {
            val stopped = NativeNodeBridge.stopRuntime(runtime.handle)
            assertTrue("stopRuntime failed: ${stopped.exceptionOrNull()}", stopped.isSuccess)
            storageDir.deleteRecursively()
        }
    }
}
