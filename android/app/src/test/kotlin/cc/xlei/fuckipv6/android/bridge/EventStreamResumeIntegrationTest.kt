package cc.xlei.fuckipv6.android.bridge

import java.util.concurrent.atomic.AtomicInteger
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.runBlocking
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * End-to-end resume: the second SSE connection must ask the node to continue
 * from the newest cursor the first one delivered.
 *
 * This is the wiring that turns `Last-Event-ID`/`?after=` from dead code into
 * behaviour, so it is asserted against a real socket rather than a fake.
 */
class EventStreamResumeIntegrationTest {
    @Test
    fun theSecondConnectionResumesFromTheLatestCursor() = runBlocking {
        val connections = AtomicInteger()
        val bodies = listOf(
            "data: {\"type\":\"event\",\"cursor\":{\"sequence\":4,\"snapshot_revision\":9}}\n",
            "data: {\"type\":\"event\",\"cursor\":{\"sequence\":5,\"snapshot_revision\":9}}\n",
        )
        TestHttpServer { _ ->
            val index = connections.getAndIncrement().coerceAtMost(bodies.lastIndex)
            TestHttpServer.Response(
                status = 200,
                body = bodies[index],
                contentType = "text/event-stream",
            )
        }.use { server ->
            val config = NodeApiConfig(baseUrl = "http://127.0.0.1:${server.port}")
            val scope = CoroutineScope(SupervisorJob() + Dispatchers.Default)
            val pulls = AtomicInteger()
            val coalescer = SnapshotFrameCoalescer(scope, { pulls.incrementAndGet() }, windowMillis = 20)

            // First stream: delivers 4:9 and closes.
            NodeEventStream(config, resumeCursor = coalescer.resumeCursor())
                .read { coalescer.onFrame(it) }
            delay(200)
            assertEquals("4:9", coalescer.resumeCursor())
            assertEquals(1, pulls.get())

            // Second stream resumes from that cursor.
            NodeEventStream(config, resumeCursor = coalescer.resumeCursor())
                .read { coalescer.onFrame(it) }
            delay(200)
            assertEquals("5:9", coalescer.resumeCursor())

            val second = server.requests[1]
            assertEquals(
                "after=" + java.net.URLEncoder.encode("4:9", "UTF-8"),
                second.query,
            )
            assertEquals("4:9", second.headers["last-event-id"])
            assertTrue(server.requests.size >= 2)
        }
    }
}
