package cc.xlei.fuckipv6.android.platform

import org.junit.Assert.assertEquals
import org.junit.Test

class NetworkRecoveryCoordinatorTest {
    @Test
    fun networkSwitchTriggersBoundedReconnectAndResetsOnSuccess() {
        val machine = BoundedRecoveryStateMachine(maxAttempts = 3, hardDeadlineMs = 10_000L)
        assertEquals(
            RecoveryDirective.TRIGGER_PATH_RECONNECT,
            machine.onEvent(RecoveryEventKind.WIFI_TO_CELLULAR, 1_000L),
        )
        assertEquals(RecoveryPhase.RECOVERING, machine.current().phase)
        assertEquals(1, machine.current().attempts)

        machine.onRecoverySucceeded(1_500L)
        assertEquals(RecoveryPhase.IDLE, machine.current().phase)
        assertEquals(0, machine.current().attempts)
    }

    @Test
    fun hardDeadlineEnforcesCleanupWithoutInfiniteLoop() {
        val machine = BoundedRecoveryStateMachine(maxAttempts = 3, hardDeadlineMs = 10_000L)
        machine.onEvent(RecoveryEventKind.LINK_ADDRESS_CHANGED, 1_000L)
        assertEquals(RecoveryDirective.NONE, machine.onTick(5_000L))
        assertEquals(RecoveryDirective.CLEANUP_AND_DEGRADE, machine.onTick(11_000L))
        assertEquals(RecoveryPhase.DEGRADED, machine.current().phase)
        assertEquals("recovery_hard_timeout", machine.current().lastReason)
    }

    @Test
    fun exhaustedAttemptsDegradeAndAirplaneModeStopsCleanly() {
        val machine = BoundedRecoveryStateMachine(maxAttempts = 2, hardDeadlineMs = 10_000L)
        assertEquals(RecoveryDirective.TRIGGER_PATH_RECONNECT, machine.onEvent(RecoveryEventKind.DOZE_EXITED, 100L))
        assertEquals(RecoveryDirective.TRIGGER_PATH_RECONNECT, machine.onEvent(RecoveryEventKind.SCREEN_WAKE, 200L))
        assertEquals(RecoveryDirective.CLEANUP_AND_DEGRADE, machine.onEvent(RecoveryEventKind.CELLULAR_TO_WIFI, 300L))
        assertEquals(RecoveryPhase.DEGRADED, machine.current().phase)

        assertEquals(RecoveryDirective.CLEANUP_AND_STOP, machine.onEvent(RecoveryEventKind.AIRPLANE_MODE_ON, 400L))
        assertEquals(RecoveryPhase.STOPPED_OFFLINE, machine.current().phase)
    }
}
