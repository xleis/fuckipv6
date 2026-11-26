package cc.xlei.fuckipv6.android.service

import org.junit.Assert.assertEquals
import org.junit.Test

class TunnelControlPlanTest {
    private fun decide(
        consent: Boolean = true,
        transport: Boolean = true,
        state: TunnelState = TunnelState.Stopped,
    ) = TunnelControlPlan.decide(consent, transport, state)

    @Test
    fun withoutATransportNothingCanBeStarted() {
        assertEquals(TunnelControl.UNAVAILABLE, decide(transport = false))
        assertEquals(
            TunnelControl.UNAVAILABLE,
            decide(consent = false, transport = false),
        )
    }

    @Test
    fun consentIsRequestedBeforeTheFirstStart() {
        assertEquals(TunnelControl.REQUEST_CONSENT, decide(consent = false))
        assertEquals(TunnelControl.START, decide(consent = true))
    }

    @Test
    fun anActiveTunnelOnlyOffersStatusOrStop() {
        assertEquals(TunnelControl.WAITING, decide(state = TunnelState.Preparing))
        assertEquals(TunnelControl.WAITING, decide(state = TunnelState.Stopping))
        assertEquals(TunnelControl.STOP, decide(state = TunnelState.Running))
    }

    @Test
    fun aFailureKeepsTheControlReachableWhenTheTransportIsBound() {
        val failed = TunnelState.Failed("state.error.detail")
        assertEquals(TunnelControl.START, decide(state = failed))
        assertEquals(
            TunnelControl.REQUEST_CONSENT,
            decide(consent = false, state = failed),
        )
        assertEquals(TunnelControl.UNAVAILABLE, decide(transport = false, state = failed))
    }
}
