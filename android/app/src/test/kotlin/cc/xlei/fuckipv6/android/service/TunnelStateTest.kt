package cc.xlei.fuckipv6.android.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TunnelStateTest {
    @Test
    fun happyPathIsStartPrepareRunStop() {
        var state: TunnelState = TunnelState.Stopped
        for (next in listOf(
            TunnelState.Preparing,
            TunnelState.Running,
            TunnelState.Stopping,
            TunnelState.Stopped,
        )) {
            state = TunnelTransitions.next(state, next) ?: state
            assertEquals(next, state)
        }
        assertFalse(state.isActive)
    }

    @Test
    fun duplicateAndOutOfOrderRequestsAreRefused() {
        assertNull(TunnelTransitions.next(TunnelState.Running, TunnelState.Running))
        assertNull(TunnelTransitions.next(TunnelState.Stopped, TunnelState.Running))
        assertNull(TunnelTransitions.next(TunnelState.Stopped, TunnelState.Stopping))
        assertNull(TunnelTransitions.next(TunnelState.Stopping, TunnelState.Preparing))

        assertEquals(
            TunnelState.Failed("x"),
            TunnelTransitions.next(TunnelState.Preparing, TunnelState.Failed("x")),
        )
        assertEquals(
            TunnelState.Stopped,
            TunnelTransitions.next(TunnelState.Failed("x"), TunnelState.Stopped),
        )
        assertTrue(TunnelState.Preparing.isActive)
    }

    @Test
    fun storeOnlyAppliesAllowedTransitions() {
        TunnelStateStore.reset()
        assertEquals(TunnelState.Preparing, TunnelStateStore.apply(TunnelState.Preparing))
        assertEquals(TunnelState.Preparing, TunnelStateStore.apply(TunnelState.Preparing))
        assertEquals(TunnelState.Running, TunnelStateStore.apply(TunnelState.Running))
        assertEquals(TunnelState.Stopped, TunnelStateStore.apply(TunnelState.Stopped))
        TunnelStateStore.reset()
    }

    @Test
    fun configValidationRejectsWhatTunCannotTake() {
        val good = TunnelConfig("家庭网络", "10.7.0.2/32", "fd00::2/128", 1400, listOf("1.1.1.1"))
        assertNull(good.validationError())

        assertEquals("state.error.detail", good.copy(networkName = " ").validationError())
        assertEquals("state.error.detail", good.copy(addressV4 = "10.7.0.2").validationError())
        assertEquals("state.error.detail", good.copy(addressV4 = "10.7.0.2/33").validationError())
        assertEquals("state.error.detail", good.copy(mtu = 100).validationError())
        assertEquals("state.error.detail", good.copy(mtu = 20_000).validationError())
        assertEquals("state.error.detail", good.copy(dnsServers = listOf(" ")).validationError())
        assertEquals("state.error.detail", good.copy(addressV6 = "fd00::2").validationError())
        assertNull(good.copy(addressV6 = null, dnsServers = emptyList()).validationError())
    }

    @Test
    fun onlyKnownServiceActionsAreHonoured() {
        assertTrue(TunnelActions.isKnown(TunnelActions.ACTION_START))
        assertTrue(TunnelActions.isKnown(TunnelActions.ACTION_STOP))
        assertFalse(TunnelActions.isKnown(null))
        assertFalse(TunnelActions.isKnown("cc.xlei.fuckipv6.android.SOMETHING"))
    }

    @Test
    fun theObservableFlowMirrorsEveryTransition() {
        TunnelStateStore.reset()
        assertEquals(TunnelState.Stopped, TunnelStateStore.updates.value)
        TunnelStateStore.apply(TunnelState.Preparing)
        assertEquals(TunnelState.Preparing, TunnelStateStore.updates.value)
        TunnelStateStore.apply(TunnelState.Running)
        assertEquals(TunnelState.Running, TunnelStateStore.updates.value)
        // A refused transition must not move the observable value either.
        TunnelStateStore.apply(TunnelState.Preparing)
        assertEquals(TunnelState.Running, TunnelStateStore.updates.value)
        TunnelStateStore.apply(TunnelState.Stopped)
        assertEquals(TunnelState.Stopped, TunnelStateStore.updates.value)
    }
}
