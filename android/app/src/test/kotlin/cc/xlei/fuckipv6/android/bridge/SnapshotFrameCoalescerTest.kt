package cc.xlei.fuckipv6.android.bridge

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SnapshotFrameCoalescerTest {
    private fun frame(
        type: String = "event",
        sequence: Long? = null,
        revision: Long? = null,
    ): NodeEventFrame = NodeEventFrame(
        type = type,
        cursor = sequence?.let { "$it:" + (revision ?: 0) },
        sequence = sequence,
        snapshotRevision = revision,
        payload = if (type == "snapshot") JSONObject().put("revision", revision ?: 0) else null,
    )

    private fun scopeOf(): CoroutineScope = CoroutineScope(SupervisorJob() + Dispatchers.Default)

    @Test
    fun aBurstOfFramesCollapsesIntoOnePull() = runBlocking {
        val pulls = AtomicInteger()
        val scope = scopeOf()
        val coalescer = SnapshotFrameCoalescer(scope, { pulls.incrementAndGet() }, windowMillis = 50)
        assertTrue(coalescer.onFrame(frame(sequence = 1)))
        assertTrue(coalescer.onFrame(frame(sequence = 2)))
        assertTrue(coalescer.onFrame(frame(sequence = 3)))
        delay(400)
        assertEquals(1, pulls.get())
        // No snapshot revision was reported, so the node's token is a bare sequence.
        assertEquals("3", coalescer.resumeCursor())
    }

    @Test
    fun aReplayedCursorIsIgnored() = runBlocking {
        val pulls = AtomicInteger()
        val scope = scopeOf()
        val coalescer = SnapshotFrameCoalescer(scope, { pulls.incrementAndGet() }, windowMillis = 20)
        coalescer.onFrame(frame(sequence = 7, revision = 2))
        delay(200)
        assertEquals(1, pulls.get())
        // The same cursor arrives again after a reconnect: no second pull.
        assertFalse(coalescer.onFrame(frame(sequence = 7, revision = 2)))
        delay(200)
        assertEquals(1, pulls.get())
        // A genuinely newer cursor refreshes again.
        assertTrue(coalescer.onFrame(frame(sequence = 8, revision = 2)))
        delay(200)
        assertEquals(2, pulls.get())
    }

    @Test
    fun framesWithoutAHandledTypeNeverPull() = runBlocking {
        val pulls = AtomicInteger()
        val scope = scopeOf()
        val coalescer = SnapshotFrameCoalescer(scope, { pulls.incrementAndGet() }, windowMillis = 20)
        assertNull(coalescer.resumeCursor())
        assertFalse(coalescer.onFrame(NodeEventFrame(type = "heartbeat")))
        delay(100)
        assertEquals(0, pulls.get())
    }

    @Test
    fun aResyncFrameAsksForAFreshPull() = runBlocking {
        val pulls = AtomicInteger()
        val scope = scopeOf()
        val coalescer = SnapshotFrameCoalescer(scope, { pulls.incrementAndGet() }, windowMillis = 20)
        assertTrue(coalescer.onFrame(NodeEventFrame(type = "resync_required")))
        delay(200)
        assertEquals(1, pulls.get())
    }

    @Test
    fun theNewestCursorSurvivesUntilAReconnectAsksForIt() = runBlocking {
        val scope = scopeOf()
        val coalescer = SnapshotFrameCoalescer(scope, { }, windowMillis = 10)
        coalescer.onFrame(frame(type = "snapshot", sequence = 1, revision = 5))
        coalescer.onFrame(frame(sequence = 2, revision = 5))
        assertEquals("2:5", coalescer.resumeCursor())
        delay(100)
    }
}
