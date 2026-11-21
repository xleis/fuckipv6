package cc.xlei.fuckipv6.android.bridge

import java.io.ByteArrayInputStream
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Real-socket tests for the File transfer client.
 *
 * Uploads are where a client can silently corrupt a user's file, so the wire
 * facts get asserted: the exact bytes, the declared length, and the query the
 * node uses to identify peer/share/path.
 */
class FileTransferClientIntegrationTest {
    private fun client(server: TestHttpServer, token: String? = "secret") =
        FileTransferClient(NodeApiConfig(baseUrl = "http://127.0.0.1:${server.port}", token = token))

    private val request = TransferRequest(
        peer = "peer-1",
        shareId = "share/1",
        path = "docs/a b.txt",
        networkId = "net-1",
        chunkSize = 4096,
    )

    @Test
    fun uploadStreamsTheExactBytesWithAFixedLength() {
        val started = """{"transfer_id":"tr-1","operation_id":"op-1","size_bytes":11}"""
        TestHttpServer { _ -> TestHttpServer.Response(status = 201, body = started) }.use { server ->
            val payload = "hello world"
            val outcome = client(server).startUpload(
                request = request,
                source = { ByteArrayInputStream(payload.toByteArray(Charsets.UTF_8)) },
                contentLength = payload.length.toLong(),
            )
            val ok = outcome as TransferOutcome.Ok
            assertEquals("tr-1", ok.value.transferId)
            assertEquals(11L, ok.value.sizeBytes)

            val sent = server.requests.single()
            assertEquals("POST", sent.method)
            assertEquals("/api/v1/file/uploads", sent.path)
            assertEquals("application/octet-stream", sent.headers["content-type"])
            assertEquals("11", sent.headers["content-length"])
            assertEquals("Bearer secret", sent.headers["authorization"])
            assertEquals(payload, sent.body)
            // The node identifies the source object from these query fields.
            assertTrue(sent.query.contains("peer=peer-1"))
            assertTrue(sent.query.contains("network_id=net-1"))
            assertTrue(sent.query.contains("share_id=share%2F1"))
            assertTrue(sent.query.contains("path=docs%2Fa+b.txt"))
            assertTrue(sent.query.contains("chunk_size=4096"))
        }
    }

    @Test
    fun aRejectedUploadIsNotReportedAsStarted() {
        TestHttpServer { _ ->
            TestHttpServer.Response(status = 403, body = "{}")
        }.use { server ->
            val outcome = client(server).startUpload(
                request = request,
                source = { ByteArrayInputStream(ByteArray(0)) },
                contentLength = 0,
            )
            assertTrue((outcome as TransferOutcome.Failed).failure is BridgeFailure.Unauthorized)
        }
    }

    @Test
    fun downloadStartPostsJsonAndReturnsAToken() {
        val pending = """
            {"token":"tok-1","transfer_id":"tr-2","operation_id":"op-2","expires_at_unix_ms":900}
        """.trimIndent()
        TestHttpServer { _ -> TestHttpServer.Response(status = 201, body = pending) }.use { server ->
            val outcome = client(server).startDownload(request)
            val ok = outcome as TransferOutcome.Ok
            assertEquals("tok-1", ok.value.token)
            assertEquals(900L, ok.value.expiresAtUnixMs)

            val sent = server.requests.single()
            assertEquals("POST", sent.method)
            assertEquals("/api/v1/file/downloads", sent.path)
            assertEquals("application/json", sent.headers["content-type"])
            assertTrue(sent.body.contains("\"share_id\":\"share/1\""))
            assertTrue(sent.body.contains("\"path\":\"docs/a b.txt\""))
            assertTrue(sent.body.contains("\"chunk_size\":4096"))
        }
    }

    @Test
    fun controlAndListingUseTheDocumentedPaths() {
        TestHttpServer { _ -> TestHttpServer.Response(status = 200, body = """{"status":"paused"}""") }.use { server ->
            val outcome = client(server).pause("tr/1", "op 1")
            assertEquals("paused", (outcome as TransferOutcome.Ok).value)
            assertEquals(
                "/api/v1/file/transfers/tr%2F1/op+1/pause",
                server.requests.single().path,
            )
        }
        val list = """
            [{"transfer_id":"tr-1","operation_id":"op-1","state":"running","transferred_bytes":10,"total_bytes":20},
             {"transfer_id":"tr-2","operation_id":"op-2","state":"failed","error_code":"file.error.digest_mismatch"}]
        """.trimIndent()
        TestHttpServer { _ -> TestHttpServer.Response(status = 200, body = list) }.use { server ->
            val outcome = client(server).listTransfers()
            val statuses = (outcome as TransferOutcome.Ok).value
            assertEquals(2, statuses.size)
            assertTrue(statuses[0].isActive)
            assertEquals(false, statuses[1].isActive)
            assertEquals("file.error.digest_mismatch", statuses[1].errorCode)
            assertEquals("/api/v1/file/transfers", server.requests.single().path)
        }
    }

    @Test
    fun aSourceShorterThanTheDeclaredLengthIsAClientBugNotANetworkFailure() {
        TestHttpServer { _ ->
            TestHttpServer.Response(status = 201, body = """{"transfer_id":"tr","operation_id":"op"}""")
        }.use { server ->
            val outcome = client(server).startUpload(
                request = request,
                source = { ByteArrayInputStream("abc".toByteArray()) },
                contentLength = 20,
            )
            val failure = (outcome as TransferOutcome.Failed).failure
            assertEquals(BridgeFailure.Misconfigured, failure)
        }
    }
}
