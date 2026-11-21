package cc.xlei.fuckipv6.android.bridge

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Real-socket test for the SSE reader: the production class builds the URL and
 * headers itself, so only an actual request proves the resume cursor, the bearer
 * token and the no-redirect rule reach the wire.
 */
class NodeEventStreamIntegrationTest {
    private fun config(server: TestHttpServer, token: String? = "secret") =
        NodeApiConfig(baseUrl = "http://127.0.0.1:${server.port}", token = token)

    @Test
    fun resumeCursorAndTokenReachTheWire() {
        val body = """
            data: {"type":"snapshot","cursor":{"sequence":1,"snapshot_revision":2},"snapshot":{"revision":2}}
            : comment line
            data: {"type":"event","cursor":{"sequence":2,"snapshot_revision":3}}

        """.trimIndent()
        TestHttpServer { _ ->
            TestHttpServer.Response(
                status = 200,
                body = body,
                contentType = "text/event-stream",
            )
        }.use { server ->
            val frames = mutableListOf<NodeEventFrame>()
            val failure = NodeEventStream(
                config = config(server),
                resumeCursor = "1:2",
            ).read { frames.add(it) }

            assertNull(failure)
            assertEquals(2, frames.size)
            assertEquals("snapshot", frames[0].type)
            assertEquals(2L, frames[1].sequence)

            val request = server.requests.single()
            assertEquals("GET", request.method)
            assertEquals("/api/v1/events", request.path)
            assertEquals("after=" + java.net.URLEncoder.encode("1:2", "UTF-8"), request.query)
            assertEquals("1:2", request.headers["last-event-id"])
            assertEquals("Bearer secret", request.headers["authorization"])
            assertEquals("text/event-stream", request.headers["accept"])
        }
    }

    @Test
    fun aMalformedCursorIsDroppedInsteadOfSendingAnInvalidResume() {
        TestHttpServer { _ ->
            TestHttpServer.Response(status = 200, body = "data: {\"type\":\"snapshot\",\"cursor\":{\"sequence\":1,\"snapshot_revision\":1}}\n", contentType = "text/event-stream")
        }.use { server ->
            val frames = mutableListOf<NodeEventFrame>()
            NodeEventStream(config(server, token = null), resumeCursor = "abc:1").read { frames.add(it) }
            assertEquals(1, frames.size)
            val request = server.requests.single()
            assertEquals("", request.query)
            assertNull(request.headers["last-event-id"])
        }
    }

    @Test
    fun unauthorizedAndServerErrorsStayDistinct() {
        TestHttpServer { _ -> TestHttpServer.Response(status = 401, body = "{}") }.use { server ->
            val failure = NodeEventStream(config(server)).read { }
            assertTrue(failure is BridgeFailure.Unauthorized)
        }
        TestHttpServer { _ -> TestHttpServer.Response(status = 503, body = "{}") }.use { server ->
            val failure = NodeEventStream(config(server)).read { }
            assertEquals(BridgeFailure.HttpStatus(503), failure)
        }
    }
}
