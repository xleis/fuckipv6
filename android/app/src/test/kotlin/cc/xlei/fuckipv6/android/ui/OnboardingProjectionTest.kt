package cc.xlei.fuckipv6.android.ui

import cc.xlei.fuckipv6.android.bridge.BridgeFailure
import cc.xlei.fuckipv6.android.bridge.BridgeState
import cc.xlei.fuckipv6.android.bridge.NodeApiConfig
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class OnboardingProjectionTest {
    @Test
    fun firstLaunchAsksForAnAddressInsteadOfShowingAnOutage() {
        val state = OnboardingProjection.decide(null, BridgeState.NotConfigured)
        assertEquals(OnboardingStep.CONFIGURE, state.step)
        assertEquals("onboarding.configure.detail", state.messageKey)
        assertTrue(state.showsForm)
    }

    @Test
    fun aBlankAddressIsStillUnconfigured() {
        val state = OnboardingProjection.decide(
            NodeApiConfig(baseUrl = "   "),
            BridgeState.NotConfigured,
        )
        assertEquals(OnboardingStep.CONFIGURE, state.step)
    }

    @Test
    fun aSavedAddressShowsProgressUntilTheNodeAnswers() {
        val config = NodeApiConfig(baseUrl = "http://127.0.0.1:7788")
        val waiting = OnboardingProjection.decide(config, BridgeState.NotConfigured)
        assertEquals(OnboardingStep.CONNECTING, waiting.step)
        assertTrue(waiting.showsForm)
        assertNull(waiting.messageKey)

        val ready = OnboardingProjection.decide(config, readyState())
        assertEquals(OnboardingStep.READY, ready.step)
        assertFalse(ready.showsForm)
    }

    @Test
    fun aRejectedAddressKeepsTheFormWithTheFailureExplanation() {
        val config = NodeApiConfig(baseUrl = "http://127.0.0.1:7788", token = "bad")
        val state = OnboardingProjection.decide(
            config,
            BridgeState.Failed(BridgeFailure.Unauthorized(401)),
        )
        assertEquals(OnboardingStep.FAILED, state.step)
        assertEquals(BridgeFailure.Unauthorized(401).uiStateKey, state.messageKey)
        assertTrue(state.showsForm)
    }

    private fun readyState(): BridgeState.Ready = BridgeState.Ready(
        data = FilePageData(
            stateKey = "state.ready.title",
            stateTone = "ready",
            networkLabel = null,
            activeTransfers = 0,
            availableBytes = null,
            formattedAvailable = null,
            accessEpoch = null,
            shareRevision = null,
            rootErrorCode = null,
            transferSurfaceKey = FilePageData.TRANSFER_SURFACE_KEY,
        ),
        snapshotRevision = 1,
    )
}
