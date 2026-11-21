package cc.xlei.fuckipv6.android.bridge

import java.io.ByteArrayOutputStream
import java.net.HttpURLConnection
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Real-socket tests for the token download path.
 *
 * A truncated or oversize body must never be reported as a saved file: the
 * operator would keep a half document they believe is complete.
 */
class HttpDownloadFetcherIntegrationTest {
    private fun fetcher(server: TestHttpServer, token: String? = "secret") =
        HttpDownloadFetcher(NodeApiConfig(baseUrl = "http://127.0.0.1:${server.port}", token = token))

    @Test
    fun aCompleteBodyIsWrittenAndReported() {
        TestHttpServer { _ ->
            TestHttpServer.Response(status = 200, body = "hello-download", contentType = "application/octet-stream")
        }.use { server ->
            val sink = ByteArrayOutputStream()
            val outcome = fetcher(server).fetch("tok:1", sink, maxBytes = 1024)
            assertEquals(FetchOutcome.Ok(14), outcome)
            assertEquals("hello-download", sink.toString(Charsets.UTF_8))
            val request = server.requests.single()
            assertEquals("GET", request.method)
            assertEquals("/api/v1/file/downloads/tok%3A1", request.path)
            assertEquals("Bearer secret", request.headers["authorization"])
            assertEquals("application/octet-stream", request.headers["accept"])
        }
    }

    @Test
    fun aDeclaredLengthAboveTheBudgetIsRefusedBeforeWriting() {
        TestHttpServer { _ ->
            TestHttpServer.Response(status = 200, body = "0123456789", contentType = "application/octet-stream")
        }.use { server ->
            val sink = ByteArrayOutputStream()
            val outcome = fetcher(server).fetch("tok", sink, maxBytes = 4)
            val failed = outcome as FetchOutcome.Failed
            assertEquals(
                BridgeFailure.HttpStatus(HttpURLConnection.HTTP_ENTITY_TOO_LARGE),
                failed.failure,
            )
            assertEquals(0, sink.size())
        }
    }

    @Test
    fun aBodyShorterThanTheDeclaredLengthIsMalformed() {
        // Content-Length says 40 while only 9 bytes are served and the socket closes.
        TestHttpServer { _ ->
            TestHttpServer.Response(
                status = 200,
                body = "short-one",
                contentType = "application/octet-stream",
                declaredLength = 40,
            )
        }.use { server ->
            val sink = ByteArrayOutputStream()
            val outcome = fetcher(server).fetch("tok", sink, maxBytes = 1024)
            assertEquals(BridgeFailure.Malformed, (outcome as FetchOutcome.Failed).failure)
            assertEquals(9, sink.size())
        }
    }

    @Test
    fun missingTokensAndRejectionsStayDistinct() {
        TestHttpServer { _ -> TestHttpServer.Response(status = 404, body = "{}") }.use { server ->
            val outcome = fetcher(server).fetch("tok", ByteArrayOutputStream(), 1024)
            assertEquals(
                BridgeFailure.HttpStatus(404),
                (outcome as FetchOutcome.Failed).failure,
            )
        }
        TestHttpServer { _ -> TestHttpServer.Response(status = 401, body = "{}") }.use { server ->
            val outcome = fetcher(server).fetch("tok", ByteArrayOutputStream(), 1024)
            assertTrue((outcome as FetchOutcome.Failed).failure is BridgeFailure.Unauthorized)
        }
    }
}
