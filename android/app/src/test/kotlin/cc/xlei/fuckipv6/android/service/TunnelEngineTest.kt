package cc.xlei.fuckipv6.android.service

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

class TunnelEngineTest {
    private val config = TunnelConfig("家庭网络", "10.7.0.2/32", "fd00::2/128", 1400, listOf("1.1.1.1"))

    private class FakeHandle : TunnelInterfaceHandle {
        var closed = 0
        override fun close() {
            closed += 1
        }
    }

    private class FakeTransport : TunnelTransportHandle {
        var startResult: Result<Unit> = Result.success(Unit)
        var starts = 0
        var stops = 0
        var lastHandle: TunnelInterfaceHandle? = null
        override fun start(config: TunnelConfig, handle: TunnelInterfaceHandle): Result<Unit> {
            starts += 1
            lastHandle = handle
            return startResult
        }
        override fun stop() {
            stops += 1
        }
    }

    @Before
    fun reset() = TunnelStateStore.reset()

    @After
    fun cleanup() = TunnelStateStore.reset()

    private fun engine(
        transport: TunnelTransportHandle?,
        handle: TunnelInterfaceHandle? = FakeHandle(),
    ) = TunnelEngine({ handle }, { transport })

    @Test
    fun startWithoutTransportFailsClosedAndNeverCreatesAnInterface() {
        var established = 0
        val engine = TunnelEngine({ established += 1; FakeHandle() }, { null })
        assertEquals(TunnelState.Failed("state.unsupported.detail"), engine.start(config))
        assertEquals(0, established)
        assertFalse(TunnelStateStore.state.isActive)
    }

    @Test
    fun invalidConfigIsRejectedBeforeAnythingIsCreated() {
        var established = 0
        val engine = TunnelEngine({ established += 1; FakeHandle() }, { FakeTransport() })
        assertEquals("state.error.detail", (engine.start(config.copy(mtu = 1)) as TunnelState.Failed).reasonKey)
        assertEquals(0, established)
    }

    @Test
    fun successfulStartReachesRunningAndStopsCleanly() {
        val handle = FakeHandle()
        val transport = FakeTransport()
        val engine = engine(transport, handle)
        assertEquals(TunnelState.Running, engine.start(config))
        assertEquals(1, transport.starts)
        assertEquals(handle, transport.lastHandle)

        assertEquals(TunnelState.Stopped, engine.stop())
        assertEquals(1, transport.stops)
        assertEquals(1, handle.closed)
    }

    @Test
    fun interfaceAndTransportFailuresCleanUpAndReport() {
        val failingTransport = FakeTransport().apply { startResult = Result.failure(IllegalStateException("no")) }
        val handle = FakeHandle()
        val engine = engine(failingTransport, handle)
        assertEquals(TunnelState.Failed("state.error.detail"), engine.start(config))
        assertEquals(1, handle.closed)
        assertEquals(1, failingTransport.stops)

        TunnelStateStore.reset()
        val noInterface = engine(FakeTransport(), handle = null)
        assertEquals(TunnelState.Failed("state.error.detail"), noInterface.start(config))
    }

    @Test
    fun duplicateStartAndStopAreNoOps() {
        val transport = FakeTransport()
        val engine = engine(transport)
        assertEquals(TunnelState.Running, engine.start(config))
        assertEquals(TunnelState.Running, engine.start(config))
        assertEquals(1, transport.starts)

        assertEquals(TunnelState.Stopped, engine.stop())
        assertEquals(TunnelState.Stopped, engine.stop())
        assertEquals(1, transport.stops)
    }

    @Test
    fun revocationIsReportedAsFailure() {
        val transport = FakeTransport()
        val engine = engine(transport)
        engine.start(config)
        assertEquals(TunnelState.Failed("state.offline.detail"), engine.revoked())
        assertEquals(1, transport.stops)
        assertFalse(TunnelStateStore.state.isActive)
        assertTrue(TunnelStateStore.state is TunnelState.Failed)
    }
}
