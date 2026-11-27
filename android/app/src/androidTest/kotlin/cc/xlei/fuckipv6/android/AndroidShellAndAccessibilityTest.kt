package cc.xlei.fuckipv6.android

import android.content.Intent
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.SemanticsMatcher
import androidx.compose.ui.test.assertHeightIsAtLeast
import androidx.compose.ui.test.assertIsDisplayed
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onFirst
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.unit.dp
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import cc.xlei.fuckipv6.android.bridge.NetworkView
import cc.xlei.fuckipv6.android.bridge.PeersSnapshot
import cc.xlei.fuckipv6.android.bridge.RoutingSummaryView
import cc.xlei.fuckipv6.android.bridge.TopologyNodeView
import cc.xlei.fuckipv6.android.bridge.TopologyView
import cc.xlei.fuckipv6.android.bridge.TrafficPolicyDocument
import cc.xlei.fuckipv6.android.bridge.TrafficRuleItem
import cc.xlei.fuckipv6.android.contract.SharedContractLoader
import cc.xlei.fuckipv6.android.platform.AndroidDevicePolicyMonitor
import cc.xlei.fuckipv6.android.platform.AndroidPasskeySessionStore
import cc.xlei.fuckipv6.android.platform.DevicePolicyEvaluator
import cc.xlei.fuckipv6.android.platform.MeteredPolicySettings
import cc.xlei.fuckipv6.android.platform.SafShareManager
import cc.xlei.fuckipv6.android.service.TunnelControl
import cc.xlei.fuckipv6.android.service.TunnelState
import cc.xlei.fuckipv6.android.ui.FilePageData
import cc.xlei.fuckipv6.android.ui.FuckIpv6App
import cc.xlei.fuckipv6.android.ui.FuckIpv6Theme
import cc.xlei.fuckipv6.android.ui.SettingsPageData
import cc.xlei.fuckipv6.android.ui.SettingsValidation
import org.json.JSONObject
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class AndroidShellAndAccessibilityTest {
    @get:Rule
    val composeRule = createComposeRule()

    @Test
    fun allFiveModulesExposeHeadingsTouchTargetsAndW16Workflows() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val contracts = SharedContractLoader.load(context.assets)
        val env = AndroidDevicePolicyMonitor(context).captureSnapshot()
        val policy = DevicePolicyEvaluator.evaluate(env, MeteredPolicySettings())
        val passkeyStore = AndroidPasskeySessionStore(context)
        val safManager = SafShareManager(context)
        val initialPolicy = TrafficPolicyDocument(
            revision = 1L,
            defaultActionKind = "direct",
            defaultActionTarget = null,
            rules = listOf(
                TrafficRuleItem("r1", "Google", true, "domain_suffix", "google.com", "exit", "node-hk", fixedExit = true),
            ),
            rawPolicy = JSONObject(),
        )
        var consentConfirmed = false
        var previewRequested = false

        composeRule.setContent {
            FuckIpv6Theme {
                FuckIpv6App(
                    contracts = contracts,
                    files = FilePageData(
                        stateKey = "status.capability.ready",
                        stateTone = "good",
                        networkLabel = "hk-exit",
                        activeTransfers = 0,
                        availableBytes = 10L * 1024L * 1024L * 1024L,
                        formattedAvailable = "10.0 GiB",
                        shareRevision = 1L,
                        accessEpoch = 1L,
                        rootErrorCode = null,
                        transferSurfaceKey = "file.page.local_state.note",
                    ),
                    settings = SettingsPageData(
                        currentUrl = "http://127.0.0.1:41234",
                        hasToken = true,
                        connected = true,
                        onSave = { _, _ -> SettingsValidation.of("http://127.0.0.1:41234", "tok") },
                        onClear = {},
                    ),
                    networks = listOf(
                        NetworkView("hk-exit", "tun0", "connected", "fd00::2", "10.8.0.2", null, 42L, listOf("exit"), 2, null),
                    ),
                    peers = PeersSnapshot("hk-exit", emptyList()),
                    routing = listOf(
                        RoutingSummaryView("hk-exit", "exit", "node-hk", "exit", "node-hk", 1, 1L, 1000L, "current", null),
                    ),
                    topology = TopologyView(
                        revision = 1L,
                        generatedAtUnixMs = 1000L,
                        nodeTruncated = false,
                        boundaryCount = 1,
                        nodes = listOf(TopologyNodeView("node-self", "S24", true, "healthy", 1)),
                        nodesTotal = 1,
                        connections = emptyList(),
                        connectionsTotal = 0,
                    ),
                    selectedNetworkName = "hk-exit",
                    tunnelControl = TunnelControl.REQUEST_CONSENT,
                    tunnelState = TunnelState.Stopped,
                    onTunnelControl = { control ->
                        if (control == TunnelControl.REQUEST_CONSENT) consentConfirmed = true
                    },
                    deviceEnv = env,
                    devicePolicy = policy,
                    trafficPolicy = initialPolicy,
                    onLoadTrafficPolicy = {},
                    onPreviewTrafficPolicy = { _, _, _ -> previewRequested = true },
                    onApplyTrafficPolicy = { _, _, _, _ -> },
                    onTestTrafficPolicy = { _, _, _, _ -> },
                    onCreateNetwork = {},
                    onJoinNetwork = { _, _ -> },
                    onRefreshDiscovery = {},
                    safShares = safManager.verifyGrants(),
                    onPickSafFolder = {},
                    onVerifySafGrants = {},
                    passkeyReauthValid = passkeyStore.hasRecentReauth(),
                    onVerifyPasskey = {},
                    onRegisterPasskey = {},
                    onClearAuthCache = { passkeyStore.clearSensitiveSessionCache() },
                )
            }
        }

        // 1. Verify all 5 bottom navigation items meet 48dp accessibility touch target.
        val headingMatcher = SemanticsMatcher.keyIsDefined(SemanticsProperties.Heading)
        for (module in contracts.navigation.modules) {
            composeRule.onNodeWithTag("module-${module.id}")
                .assertIsDisplayed()
                .assertHeightIsAtLeast(48.dp)
                .performClick()
            composeRule.onAllNodes(headingMatcher).onFirst().assertIsDisplayed()
        }

        // 2. Verify VPN pre-consent explanation dialog on Overview.
        composeRule.onNodeWithTag("module-overview").performClick()
        composeRule.onNodeWithTag("overview-device-health").assertIsDisplayed()
        composeRule.onNodeWithTag("overview-tunnel-action")
            .assertHeightIsAtLeast(48.dp)
            .performClick()
        composeRule.onNodeWithTag("vpn-consent-confirm")
            .assertIsDisplayed()
            .performClick()
        assertTrue("VPN consent callback must fire after explanation dialog", consentConfirmed)

        // 3. Verify Traffic Policy rule editor on Routing page (W11-08 #7).
        composeRule.onNodeWithTag("module-nodes").performClick()
        composeRule.onNodeWithTag("page-tab-routing")
            .assertHeightIsAtLeast(48.dp)
            .performClick()
        composeRule.onNodeWithTag("traffic-policy-card").performScrollTo().assertIsDisplayed()
        composeRule.onNodeWithTag("traffic-policy-new-domain").performScrollTo().performTextInput("youtube.com")
        composeRule.onNodeWithTag("traffic-policy-add-rule").performScrollTo().performClick()
        composeRule.onNodeWithTag("traffic-policy-preview").performScrollTo().performClick()
        composeRule.waitForIdle()
        assertTrue("Traffic policy preview callback must fire", previewRequested)

        // 4. Verify SAF card on Files page.
        composeRule.onNodeWithTag("module-activity").performClick()
        composeRule.onNodeWithTag("page-tab-files").performClick()
        composeRule.onNodeWithTag("saf-shares-card").performScrollTo().assertIsDisplayed()
        composeRule.onNodeWithTag("saf-pick-folder").performScrollTo().assertHeightIsAtLeast(48.dp)

        // 5. Verify Passkey card on Settings page & real device capability probes.
        composeRule.onNodeWithTag("module-settings").performClick()
        composeRule.onNodeWithTag("settings-passkey-card").performScrollTo().assertIsDisplayed()
        val caps = passkeyStore.capabilities()
        assertTrue("Target device must be API 28+", caps.sdkInt >= 28)
        assertTrue("Credential Manager class must be linked", caps.hasCredentialManager)
        passkeyStore.clearSensitiveSessionCache()
        assertFalse(passkeyStore.hasRecentReauth())

        val batteryIntent = AndroidDevicePolicyMonitor(context).batteryOptimizationSettingsIntent()
        assertNotNull(batteryIntent.action)
        assertTrue(batteryIntent.flags and Intent.FLAG_ACTIVITY_NEW_TASK != 0)
    }
}
