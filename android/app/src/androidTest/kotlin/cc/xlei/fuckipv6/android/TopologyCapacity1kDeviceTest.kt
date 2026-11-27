package cc.xlei.fuckipv6.android

import android.os.Build
import android.os.SystemClock
import android.util.Log
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToIndex
import androidx.compose.ui.test.performTextClearance
import androidx.compose.ui.test.performTextInput
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import cc.xlei.fuckipv6.android.bridge.TopologyConnectionView
import cc.xlei.fuckipv6.android.bridge.TopologyNodeView
import cc.xlei.fuckipv6.android.bridge.TopologyView
import cc.xlei.fuckipv6.android.contract.SharedContractLoader
import cc.xlei.fuckipv6.android.ui.FuckIpv6Theme
import cc.xlei.fuckipv6.android.ui.TopologyContent
import java.util.Locale
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Real-device 1,000-node / 5,000-connection Compose virtualized topology
 * benchmark (W9-08 #7).
 */
@RunWith(AndroidJUnit4::class)
class TopologyCapacity1kDeviceTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun rendersAndScrolls1000NodesAnd5000ConnectionsSmoothlyOnRealDevice() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val contracts = SharedContractLoader.load(context.assets)

        val nodes = (0 until 1000).map { i ->
            val id = "node-${i.toString().padStart(4, '0')}"
            TopologyNodeView(
                nodeId = id,
                displayName = "Node $i",
                isSelf = i == 0,
                health = if (i % 17 == 0) "degraded" else "healthy",
                networkCount = (i % 3) + 1,
            )
        }
        val connections = (0 until 5000).map { i ->
            val from = "node-${(i % 1000).toString().padStart(4, '0')}"
            val to = "node-${((i * 7 + 1) % 1000).toString().padStart(4, '0')}"
            TopologyConnectionView(
                from = from,
                to = to,
                state = if (i % 5 == 0) "relayed" else "direct",
                networkCount = (i % 2) + 1,
            )
        }
        val view = TopologyView(
            revision = 42L,
            generatedAtUnixMs = System.currentTimeMillis(),
            nodeTruncated = false,
            boundaryCount = 4,
            nodes = nodes,
            nodesTotal = nodes.size,
            connections = connections,
            connectionsTotal = connections.size,
        )

        val t0 = SystemClock.elapsedRealtime()
        composeRule.setContent {
            FuckIpv6Theme {
                TopologyContent(contracts = contracts, locale = Locale.US, topology = view)
            }
        }
        composeRule.onNodeWithTag("topology-summary").assertIsDisplayed()
        val firstFrameMs = SystemClock.elapsedRealtime() - t0

        // Scroll virtualized LazyColumn to the 1000th node.
        val tScrollNodes = SystemClock.elapsedRealtime()
        composeRule.onNodeWithTag("topology-virtual-list").performScrollToIndex(999)
        composeRule.onNodeWithTag("topology-node-node-0999").assertIsDisplayed()
        val scrollNodesMs = SystemClock.elapsedRealtime() - tScrollNodes

        // Filter 1,000 nodes down to a single match.
        val tFilter = SystemClock.elapsedRealtime()
        composeRule.onNodeWithTag("topology-filter").performTextInput("node-0777")
        composeRule.onNodeWithTag("topology-node-node-0777").assertIsDisplayed()
        composeRule.onNodeWithTag("topology-filter").performTextClearance()
        val filterMs = SystemClock.elapsedRealtime() - tFilter

        // Switch to 5,000 connections tab and scroll to the 5000th link.
        val tLinks = SystemClock.elapsedRealtime()
        composeRule.onNodeWithTag("topology-tab-links").performClick()
        composeRule.onNodeWithTag("topology-virtual-list").performScrollToIndex(4999)
        val lastConn = connections[4999]
        composeRule.onNodeWithTag("topology-link-${lastConn.from}-${lastConn.to}").assertIsDisplayed()
        val scrollLinksMs = SystemClock.elapsedRealtime() - tLinks

        val heapMb = (Runtime.getRuntime().totalMemory() - Runtime.getRuntime().freeMemory()) / (1024.0 * 1024.0)
        Log.i(
            "W9_08_ANDROID_BENCH",
            "model=${Build.MODEL} sdk=${Build.VERSION.SDK_INT} nodes=1000 links=5000 " +
                "first_frame_ms=$firstFrameMs scroll_nodes_ms=$scrollNodesMs filter_ms=$filterMs " +
                "scroll_links_ms=$scrollLinksMs heap_mb=${"%.1f".format(Locale.US, heapMb)}",
        )
        assertTrue("First frame render exceeded 5000ms: $firstFrameMs", firstFrameMs < 5000L)
        assertTrue("Heap usage exceeded 256MB: $heapMb", heapMb < 256.0)
    }
}
